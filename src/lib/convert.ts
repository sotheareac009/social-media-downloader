/** Bridge to the video splitter. All cutting happens in Rust, via FFmpeg. */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface VideoProbe {
  path: string;
  file_name: string;
  duration_seconds: number;
  width: number | null;
  height: number | null;
  size_bytes: number;
  has_video: boolean;
}

export interface Clip {
  index: number;
  path: string;
  start_seconds: number;
  duration_seconds: number;
  size_bytes: number;
}

export interface SplitResult {
  output_dir: string;
  clips: Clip[];
}

export interface SplitProgress {
  index: number;
  total: number;
  /** "cutting" | "done" */
  state: string;
  path: string | null;
}

/** Open the file picker. Resolves to null when it's dismissed. */
export const convertPickFile = () => invoke<string | null>("convert_pick_file");

/** Read duration and dimensions, or reject if the file isn't readable video. */
export const convertProbe = (path: string) =>
  invoke<VideoProbe>("convert_probe", { path });

/**
 * Cut a file into equal pieces — either a count, or a length per piece.
 *
 * `exact` re-encodes so every cut lands on the requested second; without it
 * the parts are copied as-is, which is far faster but starts each one at the
 * nearest keyframe.
 */
export const convertSplit = (
  path: string,
  /** Either a number of parts, or a length in seconds for each part. */
  by: { parts: number } | { seconds: number },
  exact: boolean,
  outputDir?: string,
) =>
  invoke<SplitResult>("convert_split", {
    path,
    parts: "parts" in by ? by.parts : null,
    partSeconds: "seconds" in by ? by.seconds : null,
    exact,
    outputDir: outputDir ?? null,
  });

/** Per-part progress while a split runs. */
export const subscribeToSplitProgress = (
  onProgress: (p: SplitProgress) => void,
) => listen<SplitProgress>("convert://progress", (e) => onProgress(e.payload));

/** "1 h 03 min 20 s", the way the screen states a length. */
export function formatLength(seconds: number): string {
  const total = Math.round(seconds);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (h > 0) return `${h} h ${String(m).padStart(2, "0")} min`;
  if (m > 0) return `${m} min ${String(s).padStart(2, "0")} s`;
  return `${s} s`;
}

/** Extensions the picker accepts, mirrored here to vet a dropped file. */
const VIDEO_EXTENSIONS = [
  "mp4", "mov", "mkv", "webm", "avi", "m4v", "mpg", "mpeg", "ts", "flv",
];

/**
 * Whether a dropped path looks like a video.
 *
 * A guess by extension only — Rust probes the file for real. This exists so a
 * dropped folder or screenshot is refused instantly instead of after a round
 * trip through FFmpeg.
 */
export function looksLikeVideo(path: string): boolean {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return VIDEO_EXTENSIONS.includes(ext);
}


// ------------------------------------------------------------ batch convert

export type MediaKind = "video" | "photo";

/** One row in the file table. */
export interface MediaItem {
  id: string;
  path: string;
  file_name: string;
  directory: string;
  kind: MediaKind;
  size_bytes: number;
  duration_seconds: number | null;
  width: number | null;
  height: number | null;
  fps: number | null;
  /**
   * The codecs already inside the file. These decide whether a conversion has
   * to re-encode at all — a `.ts` holding H.264 and AAC becomes an MP4 by
   * rewriting the container, with the streams copied untouched.
   */
  vcodec: string | null;
  acodec: string | null;
  /** False when FFmpeg could read nothing from the file. */
  supported: boolean;
}

/** Containers the converter can write. Any source format reaches any of these. */
export type VideoFormat = "mp4" | "mkv" | "mov" | "webm" | "avi" | "mp3";
export type PhotoFormat = "jpg" | "png" | "webp";

/** How to fit a picture into a shape it does not already have. */
export type Fit = "crop" | "blur" | "pad";

export interface Aspect {
  w: number;
  h: number;
  fit: Fit;
}

export interface ConvertSettings {
  video_format: VideoFormat;
  photo_format: PhotoFormat;
  /** Height ceiling for video, or null to keep the source resolution. */
  video_height: number | null;
  fps: number | null;
  photo_height: number | null;
  threads: number;
  gpu: boolean;
  /** A target shape from a preset, or null to keep the source's. */
  aspect: Aspect | null;
  /** Leave a file untouched when it already matches the request. */
  skip_conforming: boolean;
  delete_original: boolean;
  /** Where results land. null keeps the default folder beside each source. */
  output_dir: string | null;
}

export interface JobUpdate {
  id: string;
  /** "converting" | "done" | "skipped" | "failed" | "cancelled" */
  status: string;
  /** "copy" (streams moved untouched), "encode", or "skip". */
  how: string | null;
  percent: number | null;
  output_path: string | null;
  output_bytes: number | null;
  error: string | null;
}

export interface BatchDone {
  converted: number;
  failed: number;
  /** Files that already matched the request, so nothing was written. */
  skipped: number;
  cancelled: boolean;
}

export interface MergeProgress {
  percent: number;
  /** "copy" when the clips are appended untouched, "encode" otherwise. */
  how: string;
}

export interface MergeResult {
  path: string;
  size_bytes: number;
  duration_seconds: number;
  how: string;
}

/** The shape a merged file takes when its clips disagree. */
export type MergeShape = "first" | "landscape" | "portrait" | "square";

/**
 * Join clips into one file, in the order given.
 *
 * Order is the feature: the array is used exactly as passed, so the same clip
 * twice in a row is a legitimate request rather than a mistake to dedupe.
 */
export const convertMerge = (
  items: MediaItem[],
  outputPath: string,
  opts: { shape?: MergeShape; fit?: Fit; format?: VideoFormat; height?: number } = {},
) =>
  invoke<MergeResult>("convert_merge", {
    items,
    outputPath,
    format: opts.format ?? "mp4",
    height: opts.height ?? null,
    shape: opts.shape ?? "first",
    fit: opts.fit ?? "pad",
  });

export const subscribeToMergeProgress = (
  onProgress: (p: MergeProgress) => void,
) => listen<MergeProgress>("convert://merge", (e) => onProgress(e.payload));

export interface ConvertCapabilities {
  ffmpeg: boolean;
  /** The encoder that would actually run — "Apple VideoToolbox", "CPU (x264)". */
  encoder_label: string;
  has_hardware: boolean;
  cpu_threads: number;
  default_threads: number;
  max_threads: number;
  /** Extensions this FFmpeg build can write. Anything absent is not offered. */
  video_formats: VideoFormat[];
  photo_formats: PhotoFormat[];
}

export const convertCapabilities = () =>
  invoke<ConvertCapabilities>("convert_capabilities");

export const convertPickFolder = () =>
  invoke<string | null>("convert_pick_folder");

/** Choose several videos. Resolves to an empty array when dismissed. */
export const convertPickVideos = () => invoke<string[]>("convert_pick_videos");

/** Choose where results are written. Resolves to null when dismissed. */
export const convertPickOutputDir = () =>
  invoke<string | null>("convert_pick_output_dir");

/** Walk dropped files and folders into rows, probing each one. */
export const convertScan = (paths: string[]) =>
  invoke<MediaItem[]>("convert_scan", { paths });

/** Run the batch. Resolves when every file has finished or been cancelled. */
export const convertStart = (items: MediaItem[], settings: ConvertSettings) =>
  invoke<BatchDone>("convert_start", { items, settings });

export const convertCancel = () => invoke<void>("convert_cancel");

/** Per-file state changes while a batch runs. */
export const subscribeToConvertJobs = (onJob: (j: JobUpdate) => void) =>
  listen<JobUpdate>("convert://job", (e) => onJob(e.payload));

/** Fired once when a batch stops, cancelled or not. */
export const subscribeToConvertDone = (onDone: (d: BatchDone) => void) =>
  listen<BatchDone>("convert://done", (e) => onDone(e.payload));

/** "1920×1080", or a dash when the probe came back empty. */
export function formatResolution(
  width: number | null,
  height: number | null,
): string {
  if (!width || !height) return "—";
  return `${width}×${height}`;
}
