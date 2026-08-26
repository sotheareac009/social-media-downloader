//! Turning what was dropped into rows in the table.
//!
//! A drop can be one file, twenty files, or a folder holding hundreds. All
//! three arrive the same way, so the scan walks whatever it is given and
//! reports one row per media file it recognises.
//!
//! Every row is probed for duration, dimensions and frame rate, because a
//! table that shows "-" in those columns cannot answer the question people
//! open it with: which of these actually need converting. Probing is done
//! several files at a time, since a folder of 300 clips probed one by one is a
//! visible wait.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};

/// Video containers worth offering. Anything else is listed as unsupported
/// rather than silently dropped, so a folder's contents still add up.
/// Video containers recognised by name.
///
/// Broad on purpose: FFmpeg reads far more than the handful people usually
/// think of, and a camera or capture card writing `.mts` or `.m2ts` is a
/// perfectly ordinary source. An extension missing from this list is not a
/// refusal - a file dropped directly is probed anyway, see `scan_paths`.
const VIDEO_EXTENSIONS: &[&str] = &[
    // The common ones.
    "mp4", "mov", "mkv", "webm", "avi", "m4v", "mpg", "mpeg", "ts", "flv", "wmv", "3gp",
    // Camcorder and broadcast captures.
    "mts", "m2ts", "m2t", "trp", "tod", "mod", "dv", "dvr-ms", "mxf", "vob", "m2v", "mpe",
    "mpv", "m1v", "tp",
    // Streaming and web.
    "f4v", "f4p", "ogv", "ogm", "asf", "divx", "xvid", "3g2", "amv", "rm", "rmvb", "swf",
    "webm~", "qt", "mp4v", "h264", "hevc", "av1",
    // Phone and screen recorders.
    "mkv3d", "m4s", "mqv", "nsv", "roq", "yuv", "y4m",
];

/// Photo formats recognised by name.
const PHOTO_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "webp", "bmp", "tif", "tiff", "heic", "heif",
    "avif", "gif", "tga", "ppm", "pgm", "pbm", "pnm", "dpx", "exr", "jxl",
];

/// How many files are probed at once.
///
/// ffprobe is IO-bound and short-lived, so this is much higher than the
/// conversion concurrency - the two are not the same kind of work.
const PROBE_CONCURRENCY: usize = 8;

/// How deep a dropped folder is walked.
///
/// Deep enough for the way people organise downloads (`video/<channel>/`), and
/// shallow enough that dropping a home directory does not enumerate a disk.
const MAX_DEPTH: usize = 6;

/// A cap on rows, so one careless drop cannot lock the UI up for minutes.
const MAX_ITEMS: usize = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Video,
    Photo,
}

/// One row in the file table.
///
/// Deserialised as well as serialised: the table hands the rows it holds back
/// to `convert_start`, so the selection the user actually made is what runs,
/// not a re-scan that might see a different folder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaItem {
    /// Stable for the life of the table; the row key and the progress key.
    pub id: String,
    pub path: String,
    pub file_name: String,
    /// The containing folder, shown as its own column.
    pub directory: String,
    pub kind: MediaKind,
    pub size_bytes: u64,
    /// Absent for photos, and for a video FFmpeg could not read.
    pub duration_seconds: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Rounded to one decimal - 29.97 matters, 29.970030 does not.
    pub fps: Option<f64>,
    /// The codecs already inside the file. These decide whether a conversion
    /// has to re-encode at all: a `.ts` holding H.264 and AAC becomes an MP4
    /// by rewriting the container, with the streams copied untouched.
    pub vcodec: Option<String>,
    pub acodec: Option<String>,
    /// False when nothing could be read from the file. Kept in the table with
    /// a reason rather than hidden, so the totals still account for it.
    pub supported: bool,
}

/// Walk everything that was dropped and probe what it finds.
pub async fn scan_paths(paths: Vec<String>, ffmpeg: Option<PathBuf>) -> Vec<MediaItem> {
    // `true` means the path was handed over directly rather than found by
    // walking a folder. That distinction decides what happens to an extension
    // nobody recognises: dropping a file IS the user saying "this one", so it
    // gets probed and kept if FFmpeg can read a picture out of it. Doing the
    // same inside a folder walk would run ffprobe over every .txt and .zip on
    // the way past.
    let mut files: Vec<(PathBuf, bool)> = Vec::new();
    for raw in paths {
        let path = PathBuf::from(raw);
        if path.is_dir() {
            let mut found = Vec::new();
            collect_dir(&path, 0, &mut found);
            files.extend(found.into_iter().map(|p| (p, false)));
        } else if path.is_file() {
            files.push((path, true));
        }
        if files.len() >= MAX_ITEMS {
            break;
        }
    }

    files.sort();
    files.dedup_by(|a, b| a.0 == b.0);
    files.truncate(MAX_ITEMS);

    let ffprobe = ffmpeg
        .as_deref()
        .and_then(crate::download::compat::ffprobe_beside);

    let mut items = Vec::with_capacity(files.len());
    for chunk in files.chunks(PROBE_CONCURRENCY) {
        let mut set = tokio::task::JoinSet::new();
        for (path, explicit) in chunk {
            let path = path.clone();
            let explicit = *explicit;
            let ffprobe = ffprobe.clone();
            set.spawn(async move { probe_one(&path, ffprobe.as_deref(), explicit).await });
        }
        while let Some(res) = set.join_next().await {
            if let Ok(Some(item)) = res {
                items.push(item);
            }
        }
    }

    // Restore a stable order: JoinSet completes out of order, and a table that
    // reshuffles itself between scans is disorienting.
    items.sort_by(|a, b| a.path.cmp(&b.path));
    items
}

fn collect_dir(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH || out.len() >= MAX_ITEMS {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // Never walk into a previous run's output: converting a converted
        // folder again is the one thing a batch tool must not do by itself.
        if path.is_dir() {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string());
            if name.as_deref().is_some_and(|n| n.ends_with("(converted)")) {
                continue;
            }
            collect_dir(&path, depth + 1, out);
        } else if kind_of(&path).is_some() {
            out.push(path);
        }
        if out.len() >= MAX_ITEMS {
            return;
        }
    }
}

/// Which column a file belongs in, by extension. `None` means "not media".
pub fn kind_of(path: &Path) -> Option<MediaKind> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        Some(MediaKind::Video)
    } else if PHOTO_EXTENSIONS.contains(&ext.as_str()) {
        Some(MediaKind::Photo)
    } else {
        None
    }
}

async fn probe_one(path: &Path, ffprobe: Option<&Path>, explicit: bool) -> Option<MediaItem> {
    let known = kind_of(path);
    // An unrecognised extension is only worth a look when the file was handed
    // over directly, and only FFmpeg can settle it.
    if known.is_none() && !explicit {
        return None;
    }
    let kind = known.unwrap_or(MediaKind::Video);
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    let mut item = MediaItem {
        id: uuid::Uuid::new_v4().to_string(),
        file_name: path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        directory: path
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        path: path.display().to_string(),
        kind,
        size_bytes: size,
        duration_seconds: None,
        width: None,
        height: None,
        fps: None,
        vcodec: None,
        acodec: None,
        // Without ffprobe the file is still listed and still convertible; only
        // its details are unknown, so it is not marked unsupported.
        supported: true,
    };

    let Some(ffprobe) = ffprobe else {
        // No prober: a recognised extension is taken on trust, an unknown one
        // cannot be.
        return known.map(|_| item);
    };

    let Ok(out) = crate::process::command(ffprobe)
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=width,height,r_frame_rate,codec_name,codec_type:format=duration",
            "-of",
            "json",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .await
    else {
        return known.map(|_| item);
    };

    if !out.status.success() {
        item.supported = false;
        return known.map(|_| item);
    }

    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        item.supported = false;
        return known.map(|_| item);
    };

    // Both streams now, not just video: the audio codec decides whether the
    // sound can be copied as well, and a stream-copy is only possible when
    // both sides fit the target container.
    let streams = v.get("streams").and_then(|s| s.as_array());
    let by_kind = |kind: &str| {
        streams.and_then(|arr| {
            arr.iter().find(|s| {
                s.get("codec_type")
                    .and_then(|t| t.as_str())
                    .is_some_and(|t| t == kind)
            })
        })
    };
    let audio = by_kind("audio");
    item.acodec = audio
        .and_then(|s| s.get("codec_name"))
        .and_then(|c| c.as_str())
        .map(str::to_string);

    let stream = by_kind("video").or_else(|| streams.and_then(|a| a.first()));
    if stream.is_none() {
        // No video stream at all: an audio file with a video extension, or a
        // corrupt download. A file only guessed at by its being dropped is
        // left out entirely rather than listed as broken.
        item.supported = false;
        return known.map(|_| item);
    }

    item.width = stream
        .and_then(|s| s.get("width"))
        .and_then(|n| n.as_u64())
        .map(|n| n as u32);
    item.height = stream
        .and_then(|s| s.get("height"))
        .and_then(|n| n.as_u64())
        .map(|n| n as u32);

    item.vcodec = stream
        .and_then(|s| s.get("codec_name"))
        .and_then(|c| c.as_str())
        .map(str::to_string);

    if kind == MediaKind::Video {
        item.duration_seconds = v
            .get("format")
            .and_then(|f| f.get("duration"))
            .and_then(|d| d.as_str())
            .and_then(|d| d.parse::<f64>().ok())
            .filter(|d| d.is_finite() && *d > 0.0);
        item.fps = stream
            .and_then(|s| s.get("r_frame_rate"))
            .and_then(|r| r.as_str())
            .and_then(parse_frame_rate);
    }

    Some(item)
}

/// ffprobe reports frame rate as a fraction: "30000/1001", not "29.97".
fn parse_frame_rate(raw: &str) -> Option<f64> {
    let (num, den) = raw.split_once('/')?;
    let num: f64 = num.parse().ok()?;
    let den: f64 = den.parse().ok()?;
    // "0/0" is what a still image reports.
    if den == 0.0 || num == 0.0 {
        return None;
    }
    Some(((num / den) * 10.0).round() / 10.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_rate_fraction_becomes_the_number_people_recognise() {
        assert_eq!(parse_frame_rate("30000/1001"), Some(30.0));
        assert_eq!(parse_frame_rate("24000/1001"), Some(24.0));
        assert_eq!(parse_frame_rate("30/1"), Some(30.0));
        assert_eq!(parse_frame_rate("60/1"), Some(60.0));
        // A still image, and garbage.
        assert_eq!(parse_frame_rate("0/0"), None);
        assert_eq!(parse_frame_rate("not a fraction"), None);
    }

    #[test]
    fn extensions_decide_the_column_a_file_lands_in() {
        assert_eq!(kind_of(Path::new("/a/clip.MP4")), Some(MediaKind::Video));
        assert_eq!(kind_of(Path::new("/a/clip.mkv")), Some(MediaKind::Video));
        assert_eq!(kind_of(Path::new("/a/shot.JPG")), Some(MediaKind::Photo));
        assert_eq!(kind_of(Path::new("/a/shot.webp")), Some(MediaKind::Photo));
        // Not media, so it never reaches the table.
        assert_eq!(kind_of(Path::new("/a/notes.txt")), None);
        assert_eq!(kind_of(Path::new("/a/no-extension")), None);
    }
}
