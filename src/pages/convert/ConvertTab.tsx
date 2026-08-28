import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { Button } from "@/components/ui/Button";
import { useToast } from "@/components/ui/Toast";
import { QueuePager } from "@/components/downloads/QueuePager";
import {
  AlertIcon,
  BoltIcon,
  CheckIcon,
  FolderIcon,
  StopIcon,
  TrashIcon,
  XIcon,
} from "@/components/ui/icons";
import {
  convertCancel,
  convertCapabilities,
  convertPickFolder,
  convertPickOutputDir,
  convertScan,
  convertStart,
  formatLength,
  formatResolution,
  subscribeToConvertJobs,
  type ConvertCapabilities,
  type ConvertSettings,
  type Fit,
  type JobUpdate,
  type MediaItem,
  type MediaKind,
  type PhotoFormat,
  type VideoFormat,
} from "@/lib/convert";
import { downloadReveal, formatBytes, downloadMessage } from "@/lib/download";
import { toAuthError } from "@/lib/auth";

/**
 * Shorten a path for display, keeping the end.
 *
 * Done here rather than with CSS: `direction: rtl` truncates from the left but
 * also reorders the string, so `/Users/me/clips` renders as `Users/me/clips/`
 * with the leading slash moved to the end. Dropping whole segments keeps the
 * text honest, and the tail is the part that identifies the folder anyway.
 */
function shortPath(path: string, max = 34): string {
  if (path.length <= max) return path;
  const parts = path.split("/").filter(Boolean);
  for (let keep = 2; keep >= 1; keep--) {
    const tail = parts.slice(-keep).join("/");
    if (tail.length <= max - 2) return `…/${tail}`;
  }
  // A single segment longer than the budget: cut it rather than overflow.
  return `…${path.slice(-(max - 1))}`;
}

/** Rows per page. A folder of downloads runs to hundreds; the DOM should not. */
const PAGE_SIZE = 100;

/** Height options, as a ceiling rather than a target — nothing is upscaled. */
const VIDEO_HEIGHTS = [
  { value: 2160, label: "2160p (4K)" },
  { value: 1440, label: "1440p (2K)" },
  { value: 1080, label: "1080p" },
  { value: 720, label: "720p" },
  { value: 480, label: "480p" },
  { value: 0, label: "Keep original" },
];

const PHOTO_HEIGHTS = [
  { value: 2160, label: "2160 px" },
  { value: 1440, label: "1440 px" },
  { value: 1080, label: "1080 px" },
  { value: 720, label: "720 px" },
  { value: 0, label: "Keep original" },
];

/**
 * Output containers, with what each is actually for.
 *
 * The codecs are not a separate choice: a container only takes certain ones,
 * and picking them here rather than exposing them is the difference between a
 * file that plays and one FFmpeg writes happily but no player opens.
 */
const VIDEO_FORMATS: { value: VideoFormat; label: string; note: string }[] = [
  { value: "mp4", label: "MP4", note: "H.264 + AAC — plays everywhere" },
  { value: "mkv", label: "MKV", note: "H.264 + AAC in Matroska" },
  { value: "mov", label: "MOV", note: "H.264 + AAC, for macOS tools" },
  { value: "webm", label: "WebM", note: "VP9 + Opus — no hardware encoding" },
  { value: "avi", label: "AVI", note: "H.264 + MP3, for older software" },
  { value: "mp3", label: "MP3", note: "Audio only — the video is dropped" },
];

const PHOTO_FORMATS: { value: PhotoFormat; label: string }[] = [
  { value: "jpg", label: "JPG" },
  { value: "png", label: "PNG" },
  { value: "webp", label: "WebP" },
];

/**
 * Platform presets: one choice that sets format, size, rate and shape.
 *
 * The shape is the part the settings could not express before. A downloaded
 * YouTube video is 1920x1080 landscape; TikTok and Reels want portrait, and
 * posting the landscape file gets you a strip in the middle of the screen.
 *
 * Heights are ceilings as everywhere else - a 720p source stays 720p, it just
 * changes shape.
 */
const PRESETS: {
  id: string;
  label: string;
  format: VideoFormat;
  height: number;
  fps: number;
  aspect: { w: number; h: number } | null;
}[] = [
  // Never applied — `applyPreset` returns early for "custom" — but kept
  // truthful so it does not describe a default the page no longer has.
  { id: "custom", label: "Custom", format: "mp4", height: 0, fps: 0, aspect: null },
  { id: "tiktok", label: "TikTok / Reels — 9:16", format: "mp4", height: 1920, fps: 30, aspect: { w: 9, h: 16 } },
  { id: "shorts", label: "YouTube Shorts — 9:16", format: "mp4", height: 1920, fps: 30, aspect: { w: 9, h: 16 } },
  { id: "square", label: "Instagram feed — 1:1", format: "mp4", height: 1080, fps: 30, aspect: { w: 1, h: 1 } },
  { id: "youtube", label: "YouTube — 16:9 1080p", format: "mp4", height: 1080, fps: 30, aspect: { w: 16, h: 9 } },
];

const FITS: { value: Fit; label: string; note: string }[] = [
  { value: "crop", label: "Crop to fill", note: "Fills the frame; the sides are lost" },
  { value: "blur", label: "Blurred backdrop", note: "Whole picture, blurred copy behind it" },
  { value: "pad", label: "Black bars", note: "Whole picture, nothing cropped" },
];

const FRAME_RATES = [
  { value: 60, label: "60 fps" },
  { value: 30, label: "30 fps" },
  { value: 24, label: "24 fps" },
  { value: 0, label: "Keep original" },
];

/**
 * Batch conversion: a folder in, one consistent shape out.
 *
 * The table is the screen. Everything else — the output settings, the thread
 * count — is a small decision made once, while the table answers the question
 * people actually arrive with: what is in this folder, and which of it needs
 * converting at all.
 */
export function ConvertTab({ active }: { active: boolean }) {
  const toast = useToast();
  const [items, setItems] = useState<MediaItem[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [jobs, setJobs] = useState<Record<string, JobUpdate>>({});
  const [caps, setCaps] = useState<ConvertCapabilities | null>(null);
  const [scanning, setScanning] = useState(false);
  // Videos and photos are separate screens and separate jobs: each has its own
  // lane in Rust, so one can encode while the other runs. `running` below is
  // just the visible pane's flag, which is what every control on it cares
  // about.
  const [pane, setPane] = useState<MediaKind>("video");
  const [lanes, setLanes] = useState<Record<MediaKind, boolean>>({
    video: false,
    photo: false,
  });
  const [dragging, setDragging] = useState(false);
  const running = lanes[pane];
  const [page, setPage] = useState(0);

  const [preset, setPreset] = useState("custom");
  const [fit, setFit] = useState<Fit>("crop");
  // Left on by default: re-encoding a file that already matches the request
  // costs minutes and hands back something very slightly worse.
  const [skipConforming, setSkipConforming] = useState(true);
  const [videoFormat, setVideoFormat] = useState<VideoFormat>("mp4");
  const [photoFormat, setPhotoFormat] = useState<PhotoFormat>("jpg");
  // Keep the source's resolution and frame rate unless asked otherwise.
  //
  // 0 means "Keep original", which the Rust side receives as `None`. Defaulting
  // to a cap silently degraded every file that was already fine: a 4K clip came
  // back at 1080p, and a 60 fps one at 30 — losses nobody chose and nothing
  // undoes. It also forced a full re-encode where a container rewrite would
  // have done, so the safe default is the fast one too.
  const [videoHeight, setVideoHeight] = useState(0);
  const [fps, setFps] = useState(0);
  const [photoHeight, setPhotoHeight] = useState(0);
  // Per lane, not shared: videos and photos run at the same time now, so one
  // number would be a budget split between two batches without saying so.
  // Photos are cheap and IO-bound, videos are neither, which is why they do
  // not want the same value.
  const [threads, setThreads] = useState<Record<MediaKind, number>>({
    video: 2,
    photo: 4,
  });
  const [gpu, setGpu] = useState(true);
  const [deleteOriginal, setDeleteOriginal] = useState(false);
  // null means the default: a "(converted)" folder beside each source folder,
  // which keeps a multi-folder drop organised. A chosen folder collects
  // everything in one place instead.
  const [outDir, setOutDir] = useState<string | null>(null);

  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  // Mirrors `items` so a scan can dedupe against the current table without
  // taking `items` as a dependency, which would rebuild the drop listener on
  // every add.
  const itemsRef = useRef<MediaItem[]>([]);
  useEffect(() => {
    itemsRef.current = items;
  }, [items]);

  useEffect(() => {
    void convertCapabilities()
      .then((c) => {
        if (!mounted.current) return;
        setCaps(c);
        setThreads((prev) => ({ ...prev, video: c.default_threads }));
        // A format this build cannot write must not stay selected; the first
        // one it reports is always something it can produce.
        if (c.video_formats.length > 0 && !c.video_formats.includes("mp4")) {
          setVideoFormat(c.video_formats[0]);
        }
        if (c.photo_formats.length > 0 && !c.photo_formats.includes("jpg")) {
          setPhotoFormat(c.photo_formats[0]);
        }
        // Offering hardware acceleration this build cannot perform would be a
        // lie the first conversion exposes.
        setGpu(c.has_hardware);
      })
      .catch(() => {});
  }, []);

  /** Probe whatever was dropped or picked, and add it to the table. */
  const ingest = useCallback(
    async (paths: string[]) => {
      if (paths.length === 0) return;
      setScanning(true);
      try {
        const found = await convertScan(paths);
        if (!mounted.current) return;
        if (found.length === 0) {
          toast("info", "No videos or photos in there.");
          return;
        }
        // Re-dropping a folder should refresh it, not double it. Read the
        // current rows from the ref rather than from inside a state updater,
        // which must stay free of side effects.
        const known = new Set(itemsRef.current.map((i) => i.path));
        const added = found.filter((f) => !known.has(f.path));
        if (added.length === 0) {
          toast("info", "Those files are already in the table.");
          return;
        }
        setItems((prev) => [...prev, ...added]);
        // Deliberately NOT selected: converting is destructive enough - it
        // writes files, and can delete the originals - that it should act on
        // what was actually ticked, never on whatever a drop happened to pull
        // in. The header checkbox takes everything in one click.
        toast(
          "success",
          `Added ${added.length} file${added.length === 1 ? "" : "s"} — tick the ones to convert.`,
        );
      } catch (e) {
        if (!mounted.current) return;
        const err = toAuthError(e);
        toast("error", downloadMessage(err.code, err.message));
      } finally {
        if (mounted.current) setScanning(false);
      }
    },
    [toast],
  );

  // Native drops. The DOM's own drag events never carry a real path in a
  // webview, so this is the only way to learn what was dropped.
  useEffect(() => {
    // Every tab stays mounted so its work survives a tab switch, so only the
    // visible one may claim a drop - two live listeners would both act on the
    // same files.
    if (!active) {
      setDragging(false);
      return;
    }
    const pending = getCurrentWebview().onDragDropEvent((event) => {
      if (!mounted.current) return;
      // Tauri fires four of these, and `enter` carries `paths` exactly like
      // `drop` does. Matching on "drop" explicitly - rather than treating
      // everything that is not `over`/`leave` as a drop - is what stops files
      // being added while they are still hovering over the window.
      switch (event.payload.type) {
        case "enter":
        case "over":
          setDragging(true);
          return;
        case "leave":
          setDragging(false);
          return;
        case "drop":
          setDragging(false);
          void ingest(event.payload.paths ?? []);
          return;
        default:
          return;
      }
    });
    return () => {
      void pending.then((un) => un());
    };
  }, [active, ingest]);

  // Per-file progress while a batch runs.
  useEffect(() => {
    const pending = subscribeToConvertJobs((job) => {
      if (mounted.current) setJobs((prev) => ({ ...prev, [job.id]: job }));
    });
    return () => {
      void pending.then((un) => un());
    };
  }, []);

  const chooseFolder = useCallback(async () => {
    try {
      const dir = await convertPickFolder();
      if (dir) await ingest([dir]);
    } catch (e) {
      toast("error", toAuthError(e).message);
    }
  }, [ingest, toast]);

  const chooseOutDir = useCallback(async () => {
    try {
      const dir = await convertPickOutputDir();
      if (dir) setOutDir(dir);
    } catch (e) {
      toast("error", toAuthError(e).message);
    }
  }, [toast]);

  const counts = useMemo(() => {
    let videos = 0;
    let photos = 0;
    let unsupported = 0;
    for (const item of items) {
      if (!item.supported) unsupported++;
      else if (item.kind === "video") videos++;
      else photos++;
    }
    const done = Object.values(jobs).filter((j) => j.status === "done").length;
    const failed = Object.values(jobs).filter((j) => j.status === "failed").length;
    return { videos, photos, unsupported, done, failed };
  }, [items, jobs]);

  const activePreset = PRESETS.find((p) => p.id === preset) ?? PRESETS[0];

  /** Applying a preset sets every output field it covers, in one go. */
  const applyPreset = (id: string) => {
    setPreset(id);
    const p = PRESETS.find((x) => x.id === id);
    if (!p || id === "custom") return;
    setVideoFormat(p.format);
    setVideoHeight(p.height);
    setFps(p.fps);
  };

  // Only offer what this FFmpeg build can actually write: a Homebrew build
  // without libwebp, for instance, fails every WebP file at conversion time.
  // Before the probe returns, show everything rather than an empty picker.
  const videoOptions = caps
    ? VIDEO_FORMATS.filter((f) => caps.video_formats.includes(f.value))
    : VIDEO_FORMATS;
  const photoOptions = caps
    ? PHOTO_FORMATS.filter((f) => caps.photo_formats.includes(f.value))
    : PHOTO_FORMATS;

  // MP3 output has no picture, so resolution and frame rate would be settings
  // with nothing to act on.
  const audioOnly = videoFormat === "mp3";
  const formatNote =
    VIDEO_FORMATS.find((f) => f.value === videoFormat)?.note ?? "";

  /** Everything of the kind this pane is about. */
  const paneItems = useMemo(() => items.filter((i) => i.kind === pane), [items, pane]);

  const chosen = useMemo(
    () => paneItems.filter((i) => selected.has(i.id) && i.supported),
    [paneItems, selected],
  );

  const pageCount = Math.max(1, Math.ceil(paneItems.length / PAGE_SIZE));
  const currentPage = Math.min(page, pageCount - 1);
  const pageStart = currentPage * PAGE_SIZE;
  const pageItems = paneItems.slice(pageStart, pageStart + PAGE_SIZE);
  useEffect(() => {
    if (page !== currentPage) setPage(currentPage);
  }, [page, currentPage]);

  // A page number from the other pane means nothing here.
  useEffect(() => {
    setPage(0);
  }, [pane]);

  const allOnPageSelected =
    pageItems.length > 0 && pageItems.every((i) => !i.supported || selected.has(i.id));

  const toggleAll = () => {
    setSelected((prev) => {
      const next = new Set(prev);
      for (const item of pageItems) {
        if (!item.supported) continue;
        if (allOnPageSelected) next.delete(item.id);
        else next.add(item.id);
      }
      return next;
    });
  };

  const start = useCallback(async () => {
    if (chosen.length === 0) return;
    const settings: ConvertSettings = {
      video_format: videoFormat,
      photo_format: photoFormat,
      video_height: videoHeight || null,
      fps: fps || null,
      photo_height: photoHeight || null,
      threads: threads[pane],
      gpu,
      aspect: activePreset?.aspect
        ? { ...activePreset.aspect, fit }
        : null,
      skip_conforming: skipConforming,
      delete_original: deleteOriginal,
      output_dir: outDir,
    };
    const lane = pane;
    setLanes((prev) => ({ ...prev, [lane]: true }));
    // Clear this pane's rows from the previous run, leaving the other pane's
    // results alone — it may still be running.
    setJobs((prev) => {
      const next = { ...prev };
      for (const item of chosen) delete next[item.id];
      return next;
    });
    try {
      const result = await convertStart(chosen, settings, lane);
      if (!mounted.current) return;
      if (result.cancelled) {
        toast("info", `Cancelled — ${result.converted} finished before stopping.`);
      } else if (result.failed > 0) {
        toast("error", `${result.converted} converted, ${result.failed} failed.`);
      } else {
        toast(
          "success",
          `Converted ${result.converted} file${result.converted === 1 ? "" : "s"}.`,
        );
      }
    } catch (e) {
      if (!mounted.current) return;
      const err = toAuthError(e);
      toast("error", downloadMessage(err.code, err.message));
    } finally {
      if (mounted.current) setLanes((prev) => ({ ...prev, [lane]: false }));
    }
  }, [
    pane,
    threads,
    chosen,
    activePreset,
    fit,
    skipConforming,
    videoFormat,
    photoFormat,
    videoHeight,
    fps,
    photoHeight,
    threads,
    gpu,
    deleteOriginal,
    outDir,
    toast,
  ]);

  const firstOutput = Object.values(jobs).find((j) => j.output_path)?.output_path;

  return (
    <div className="conv">
      <div className="conv__intake">
        <button
          type="button"
          className={`conv__drop ${dragging ? "conv__drop--over" : ""}`.trim()}
          onClick={() => void chooseFolder()}
          disabled={scanning || running}
        >
          <span className="conv__dropicon">
            <FolderIcon size={20} />
          </span>
          <span className="conv__droptext">
            {scanning
              ? "Reading files…"
              : dragging
                ? "Drop to add"
                : "Drop a folder or files here, or click to browse"}
          </span>
          <span className="conv__drophint">
            Videos and photos, including everything in sub-folders
          </span>
        </button>

        <label className={`conv__danger ${deleteOriginal ? "conv__danger--on" : ""}`.trim()}>
          <input
            type="checkbox"
            checked={deleteOriginal}
            disabled={running}
            onChange={(e) => setDeleteOriginal(e.target.checked)}
          />
          <span>
            <strong>Delete original after converting</strong>
            <em>Only after a file converts — a failure never deletes its source.</em>
          </span>
        </label>
      </div>

      <div className="tabs tabs--sub" role="tablist">
        {(["video", "photo"] as MediaKind[]).map((kind) => (
          <button
            key={kind}
            className={`tab ${pane === kind ? "tab--active" : ""}`.trim()}
            type="button"
            role="tab"
            aria-selected={pane === kind}
            onClick={() => setPane(kind)}
          >
            {kind === "video" ? "Videos" : "Photos"}
            <span className="conv__badge">
              {items.filter((i) => i.kind === kind).length}
            </span>
            {/* A lane that is working says so from the tab you are not on. */}
            {lanes[kind] && <span className="tab__dot" title="Converting" />}
          </button>
        ))}
      </div>

{/* Above the table, not below it: the button acts on the ticked rows, and
          a long list otherwise pushed it off-screen just as it was needed. */}
      <div className="conv__actions">
        <Button
          onClick={() => void start()}
          loading={running}
          disabled={chosen.length === 0 || caps?.ffmpeg === false}
          icon={<BoltIcon size={15} />}
        >
          {running
            ? `Converting ${counts.done + 1} of ${chosen.length}…`
            : chosen.length === 0
              ? "Convert selected"
              : `Convert ${chosen.length} selected file${chosen.length === 1 ? "" : "s"}`}
        </Button>
        {!running && chosen.length === 0 && paneItems.length > 0 && (
          <span className="conv__muted">
            Tick the rows you want — the box in the header takes the whole page.
          </span>
        )}
        {running && (
          <button
            className="btn btn--ghost btn--sm"
            type="button"
            onClick={() => void convertCancel(pane)}
          >
            <StopIcon size={13} />
            Stop
          </button>
        )}
        {!running && firstOutput && (
          <button
            className="btn btn--ghost btn--sm"
            type="button"
            onClick={() => void downloadReveal(outDir ?? firstOutput)}
          >
            <FolderIcon size={13} />
            Open output folder
          </button>
        )}
        {/* One control, not four. The row is ~780px wide, and a label plus a
            full path plus two buttons could not share it with Convert and
            Open — it wrapped and took its divider with it. This says the same
            thing in a third of the width. */}
        <div className="conv__dest">
          <button
            className="conv__destbtn"
            type="button"
            disabled={running}
            onClick={() => void chooseOutDir()}
            title={
              outDir
                ? `Saving to ${outDir} — click to change`
                : "Each folder gets its own “(converted)” folder next to it — click to choose one instead"
            }
          >
            <FolderIcon size={13} />
            <span className="conv__destbtn__path">
              {outDir ? shortPath(outDir, 26) : "Saved to"}
            </span>
          </button>
          {outDir && (
            <button
              className="conv__destreset"
              type="button"
              disabled={running}
              onClick={() => setOutDir(null)}
              title="Back to saving beside each source folder"
              aria-label="Reset output folder"
            >
              <XIcon size={12} />
            </button>
          )}
        </div>

        {caps?.ffmpeg === false && (
          <span className="conv__warn">
            <AlertIcon size={13} /> FFmpeg isn't installed — set it up from the
            Home page first.
          </span>
        )}
      </div>

      <section className="conv__panel">
        <header className="conv__tablehead">
          <h2 className="conv__tabletitle">
            {pane === "video" ? "Videos" : "Photos"}
            {paneItems.length > 0 && (
              <span className="conv__badge">{paneItems.length}</span>
            )}
          </h2>
          <div className="conv__tableactions">
            {items.length > 0 && (
              <span className="conv__selected">{chosen.length} selected</span>
            )}
            <button
              className="btn btn--ghost btn--sm"
              type="button"
              disabled={paneItems.length === 0 || running}
              // Clears this pane only: the other one may be mid-batch, and
              // emptying its table under it would be alarming.
              onClick={() => {
                const gone = new Set(paneItems.map((i) => i.id));
                setItems((prev) => prev.filter((i) => !gone.has(i.id)));
                setSelected((prev) => new Set([...prev].filter((id) => !gone.has(id))));
                setJobs((prev) => {
                  const next = { ...prev };
                  for (const id of gone) delete next[id];
                  return next;
                });
                setPage(0);
              }}
            >
              <TrashIcon size={13} />
              Clear all
            </button>
          </div>
        </header>

        {paneItems.length === 0 ? (
          <p className="conv__empty">
            {items.length === 0
              ? "Nothing queued yet. Drop a folder above and every video and photo inside it appears here, ready to tick."
              : `No ${pane === "video" ? "videos" : "photos"} in what you dropped — check the other tab.`}
          </p>
        ) : (
          <>
            <div className="conv__tablewrap">
              <table className="conv__table">
                <thead>
                  <tr>
                    <th className="conv__col-check">
                      <input
                        type="checkbox"
                        checked={allOnPageSelected}
                        onChange={toggleAll}
                        disabled={running}
                        aria-label="Select every file on this page"
                      />
                    </th>
                    <th className="conv__col-num">#</th>
                    {/* Status leads the row: while a batch runs it is the only
                        column anyone is reading, and at the far right it sat
                        off the edge on a narrow window. */}
                    <th className="conv__col-status">Status</th>
                    <th>Filename</th>
                    <th className="conv__col-type">Type</th>
                    <th>Folder</th>
                    <th className="conv__col-num">Duration</th>
                    <th className="conv__col-num">Size</th>
                    <th className="conv__col-num">Resolution</th>
                    <th className="conv__col-num">FPS</th>
                  </tr>
                </thead>
                <tbody>
                  {pageItems.map((item, i) => {
                    const job = jobs[item.id];
                    const on = selected.has(item.id) && item.supported;
                    return (
                      <tr
                        key={item.id}
                        className={`${on ? "conv__row--on" : ""} ${
                          item.supported ? "" : "conv__row--off"
                        } ${job?.status === "converting" ? "conv__row--busy" : ""}`.trim()}
                      >
                        <td className="conv__col-check">
                          <input
                            type="checkbox"
                            checked={on}
                            disabled={!item.supported || running}
                            onChange={() =>
                              setSelected((prev) => {
                                const next = new Set(prev);
                                if (next.has(item.id)) next.delete(item.id);
                                else next.add(item.id);
                                return next;
                              })
                            }
                            aria-label={`Convert ${item.file_name}`}
                          />
                        </td>
                        <td className="conv__col-num conv__muted">
                          {pageStart + i + 1}
                        </td>
                        <td className="conv__col-status">
                          <StatusCell item={item} job={job} />
                        </td>
                        <td
                          className="conv__name"
                          title={item.unsupported_reason ?? item.file_name}
                        >
                          {item.file_name}
                          {/* Why this row is greyed out, on the row itself. A
                              bare "unsupported" sends people looking at the
                              wrong thing — over a limit and corrupt need
                              different responses. */}
                          {!item.supported && item.unsupported_reason && (
                            <span className="conv__why">{item.unsupported_reason}</span>
                          )}
                        </td>
                        <td className="conv__col-type">
                          <span className={`conv__kind conv__kind--${item.kind}`}>
                            {item.kind}
                          </span>
                        </td>
                        <td className="conv__dir" title={item.directory}>
                          {item.directory}
                        </td>
                        <td className="conv__col-num">
                          {item.duration_seconds
                            ? formatLength(item.duration_seconds)
                            : "—"}
                        </td>
                        <td className="conv__col-num">{formatBytes(item.size_bytes)}</td>
                        <td className="conv__col-num">
                          {formatResolution(item.width, item.height)}
                        </td>
                        <td className="conv__col-num">{item.fps ?? "—"}</td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
            {pageCount > 1 && (
              <QueuePager
                page={currentPage}
                pageCount={pageCount}
                from={pageStart + 1}
                to={pageStart + pageItems.length}
                total={items.length}
                onPage={setPage}
              />
            )}
          </>
        )}
      </section>

      <div className="conv__settings">
        {/* Each pane carries only the settings that act on it: a photo batch has
            nothing to do with frame rate, and a resolution box that does
            nothing is worse than one that is absent. */}
        {pane === "video" && (
          <section className="conv__card">
          <h3 className="conv__cardtitle">Video output</h3>
          <label className="conv__field">
            <span>Preset</span>
            <select
              className="input"
              value={preset}
              disabled={running}
              onChange={(e) => applyPreset(e.target.value)}
            >
              {PRESETS.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.label}
                </option>
              ))}
            </select>
          </label>
          {activePreset.aspect && (
            <label className="conv__field">
              <span>Fit</span>
              <select
                className="input"
                value={fit}
                disabled={running}
                onChange={(e) => setFit(e.target.value as Fit)}
              >
                {FITS.map((f) => (
                  <option key={f.value} value={f.value}>
                    {f.label}
                  </option>
                ))}
              </select>
            </label>
          )}
          <label className="conv__field">
            <span>Format</span>
            <select
              className="input"
              value={videoFormat}
              disabled={running}
              onChange={(e) => setVideoFormat(e.target.value as VideoFormat)}
            >
              {videoOptions.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </select>
          </label>
          <label className="conv__field" hidden={audioOnly}>
            <span>Resolution</span>
            <select
              className="input"
              value={videoHeight}
              disabled={running}
              onChange={(e) => setVideoHeight(Number(e.target.value))}
            >
              {VIDEO_HEIGHTS.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </select>
          </label>
          <label className="conv__field" hidden={audioOnly}>
            <span>Frame rate</span>
            <select
              className="input"
              value={fps}
              disabled={running}
              onChange={(e) => setFps(Number(e.target.value))}
            >
              {FRAME_RATES.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </select>
          </label>

          <label className="conv__field">
            <span>Files at once</span>
            <input
              className="input"
              type="number"
              min={1}
              max={caps?.max_threads ?? 16}
              value={threads.video}
              disabled={running}
              onChange={(e) =>
                setThreads((prev) => ({ ...prev, video: Number(e.target.value) }))
              }
            />
          </label>
          <label className="conv__toggle">
            <input
              type="checkbox"
              checked={gpu}
              disabled={running || !caps?.has_hardware || audioOnly || videoFormat === "webm"}
              onChange={(e) => setGpu(e.target.checked)}
            />
            <span>
              <strong>Hardware acceleration</strong>
              <em>
                {!caps
                  ? "Checking…"
                  : audioOnly
                    ? "Not used for audio-only output"
                    : videoFormat === "webm"
                      ? "WebM is VP9, which the H.264 hardware encoders can't write — this batch uses the CPU"
                      : caps.has_hardware
                        ? `${caps.encoder_label} · ${caps.cpu_threads} CPU threads`
                        : `Not available in this FFmpeg build — using ${caps.encoder_label}`}
              </em>
            </span>
          </label>
          <label className="conv__toggle" style={{ marginTop: 12 }}>
            <input
              type="checkbox"
              checked={skipConforming}
              disabled={running}
              onChange={(e) => setSkipConforming(e.target.checked)}
            />
            <span>
              <strong>Don't re-encode files already correct</strong>
              <em>
                A file that already matches every setting is copied to the
                output folder as-is, rather than rebuilt into something
                slightly worse.
              </em>
            </span>
          </label>
          <p className="conv__note">
            {activePreset.aspect
              ? `${FITS.find((f) => f.value === fit)?.note}. `
              : ""}
            {formatNote}
            {" FFmpeg already spreads one file across cores, so more files at once is not automatically faster — past a handful they compete."}
            {audioOnly
              ? ""
              : " Resolution is a ceiling, never a target — a 720p source stays 720p rather than being upscaled into a bigger file with the same detail."}
          </p>
        </section>
        )}

        {pane === "photo" && (
          <section className="conv__card">
          <h3 className="conv__cardtitle">Photo output</h3>
          <label className="conv__field">
            <span>Format</span>
            <select
              className="input"
              value={photoFormat}
              disabled={running}
              onChange={(e) => setPhotoFormat(e.target.value as PhotoFormat)}
            >
              {photoOptions.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </select>
          </label>
          <label className="conv__field">
            <span>Resolution</span>
            <select
              className="input"
              value={photoHeight}
              disabled={running}
              onChange={(e) => setPhotoHeight(Number(e.target.value))}
            >
              {PHOTO_HEIGHTS.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </select>
          </label>

          <label className="conv__field">
            <span>Files at once</span>
            <input
              className="input"
              type="number"
              min={1}
              max={caps?.max_threads ?? 16}
              value={threads.photo}
              disabled={running}
              onChange={(e) =>
                setThreads((prev) => ({ ...prev, photo: Number(e.target.value) }))
              }
            />
          </label>
          <p className="conv__note">
            Photos are re-saved at high quality — PNG stays lossless, JPG and
            WebP are near-lossless at these settings. They convert quickly, so
            several at once costs little.
          </p>
        </section>
        )}

        <section className="conv__card">
          <h3 className="conv__cardtitle">Media information</h3>
          <ul className="conv__stats">
            <li>
              <span>Videos</span>
              <strong>{counts.videos}</strong>
            </li>
            <li>
              <span>Photos</span>
              <strong>{counts.photos}</strong>
            </li>
            <li>
              <span>Unreadable</span>
              <strong className={counts.unsupported > 0 ? "conv__bad" : ""}>
                {counts.unsupported}
              </strong>
            </li>
            <li>
              <span>Converted</span>
              <strong className={counts.done > 0 ? "conv__good" : ""}>
                {counts.done}
              </strong>
            </li>
            {counts.failed > 0 && (
              <li>
                <span>Failed</span>
                <strong className="conv__bad">{counts.failed}</strong>
              </li>
            )}
          </ul>
        </section>


      </div>

    </div>
  );
}

/** One row's status: a bar while it runs, an outcome after. */
function StatusCell({ item, job }: { item: MediaItem; job?: JobUpdate }) {
  if (!item.supported) {
    return <span className="pill pill--off">Unreadable</span>;
  }
  if (!job) return <span className="conv__muted">—</span>;

  if (job.status === "converting") {
    const pct = Math.round(job.percent ?? 0);
    // A copy has no percentage worth reporting - it finishes in about a second
    // - so it gets a moving bar rather than a number that jumps 0 to 100.
    const copying = job.how === "copy";
    return (
      <div
        className="conv__prog"
        title={copying ? "Copying streams — no re-encode" : `${pct}%`}
      >
        <span className="conv__prog__label">
          {copying ? "Copying" : `${pct}%`}
        </span>
        <span
          className={`conv__prog__track ${copying ? "conv__prog__track--indeterminate" : ""}`.trim()}
        >
          <span
            className="conv__prog__fill"
            style={copying ? undefined : { width: `${pct}%` }}
          />
        </span>
      </div>
    );
  }

  if (job.status === "done") {
    return (
      <span
        className="pill pill--ok"
        title={
          job.how === "clone"
            ? "Already matched every setting — copied across untouched"
            : job.how === "copy"
              ? "Streams copied into the new container — no re-encode, no quality loss"
              : "Re-encoded"
        }
      >
        <CheckIcon size={11} />
        {job.how === "clone" && <span className="pill__how">Copied</span>}
        {job.how === "copy" && <span className="pill__how">Remuxed</span>}
        {job.output_bytes ? formatBytes(job.output_bytes) : "Done"}
      </span>
    );
  }
  if (job.status === "cancelled") {
    return <span className="pill pill--off">Stopped</span>;
  }
  return (
    <span className="pill pill--bad" title={job.error ?? undefined}>
      <AlertIcon size={11} />
      Failed
    </span>
  );
}
