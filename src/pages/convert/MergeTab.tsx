import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { Button } from "@/components/ui/Button";
import { useToast } from "@/components/ui/Toast";
import {
  ArrowLeftIcon,
  BoltIcon,
  CheckIcon,
  FolderIcon,
  StopIcon,
  XIcon,
} from "@/components/ui/icons";
import {
  convertCancel,
  convertMerge,
  convertPickOutputDir,
  convertPickVideos,
  convertScan,
  formatLength,
  formatResolution,
  subscribeToMergeProgress,
  type Fit,
  type MediaItem,
  type MergeResult,
  type MergeShape,
} from "@/lib/convert";
import { downloadReveal, formatBytes, downloadMessage } from "@/lib/download";
import { toAuthError } from "@/lib/auth";

/**
 * Join clips into one file.
 *
 * Order is the whole interface: the list plays top to bottom, and the arrows
 * are how you say so. Nothing is sorted or de-duplicated behind your back —
 * the same clip twice in a row is a real thing to want.
 *
 * Whether this costs seconds or minutes depends entirely on whether the clips
 * match, so that verdict is shown before you press anything rather than
 * discovered afterwards.
 */
export function MergeTab({ active }: { active: boolean }) {
  const toast = useToast();
  const [clips, setClips] = useState<MediaItem[]>([]);
  const [scanning, setScanning] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [busy, setBusy] = useState(false);
  const [percent, setPercent] = useState(0);
  const [result, setResult] = useState<MergeResult | null>(null);
  const [name, setName] = useState("merged");
  // What shape the finished video has, and what fills the space when a clip
  // does not match it. Only matters once the clips disagree, which is why the
  // controls appear then.
  const [shape, setShape] = useState<MergeShape>("first");
  const [fit, setFit] = useState<Fit>("pad");
  const [outDir, setOutDir] = useState<string | null>(null);

  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const clipsRef = useRef<MediaItem[]>([]);
  useEffect(() => {
    clipsRef.current = clips;
  }, [clips]);

  const add = useCallback(
    async (paths: string[]) => {
      if (paths.length === 0) return;
      setScanning(true);
      try {
        const found = await convertScan(paths);
        if (!mounted.current) return;
        // Videos only: a photo has no duration to place in a timeline.
        const videos = found.filter((f) => f.kind === "video" && f.supported);
        if (videos.length === 0) {
          toast("error", "No videos in there — merging needs video files.");
          return;
        }
        // Appended, not merged into a sorted set: new clips go on the end,
        // where the arrows can move them.
        setClips((prev) => [...prev, ...videos]);
        setResult(null);
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
      if (event.payload.type === "enter" || event.payload.type === "over") {
        setDragging(true);
        return;
      }
      if (event.payload.type !== "drop") {
        setDragging(false);
        return;
      }
      setDragging(false);
      void add(event.payload.paths ?? []);
    });
    return () => {
      void pending.then((un) => un());
    };
  }, [active, add]);

  useEffect(() => {
    const pending = subscribeToMergeProgress((p) => {
      if (mounted.current) setPercent(p.percent);
    });
    return () => {
      void pending.then((un) => un());
    };
  }, []);

  const chooseClips = useCallback(async () => {
    try {
      const picked = await convertPickVideos();
      // The picker returns its own order, which the list then honours; the
      // arrows are what decides sequence from here.
      if (picked.length > 0) await add(picked);
    } catch (e) {
      toast("error", toAuthError(e).message);
    }
  }, [add, toast]);

  const choose = useCallback(async () => {
    try {
      const dir = await convertPickOutputDir();
      if (dir) setOutDir(dir);
    } catch (e) {
      toast("error", toAuthError(e).message);
    }
  }, [toast]);

  const move = (index: number, by: number) => {
    setClips((prev) => {
      const next = [...prev];
      const target = index + by;
      if (target < 0 || target >= next.length) return prev;
      [next[index], next[target]] = [next[target], next[index]];
      return next;
    });
  };

  /**
   * Mirrors `can_copy` in Rust: every property that would change mid-file has
   * to match, or a player would meet a new resolution at the join.
   */
  const identical = useMemo(() => {
    if (clips.length < 2) return false;
    const [first] = clips;
    return clips.every(
      (c) =>
        c.vcodec === first.vcodec &&
        c.acodec === first.acodec &&
        c.width === first.width &&
        c.height === first.height &&
        c.fps !== null &&
        first.fps !== null &&
        Math.abs((c.fps ?? 0) - (first.fps ?? 0)) < 0.01,
    );
  }, [clips]);

  /** Portrait and landscape in the same list: the case with no right default. */
  const mixedOrientation = useMemo(() => {
    const shapes = new Set(
      clips
        .filter((c) => c.width && c.height)
        .map((c) => ((c.width ?? 0) >= (c.height ?? 0) ? "wide" : "tall")),
    );
    return shapes.size > 1;
  }, [clips]);

  // A shape the clips do not already have means every frame is redrawn, so the
  // verdict has to account for it — the same rule Rust applies.
  const fast = identical && (shape === "first" || !mixedOrientation);

  const totalSeconds = clips.reduce((sum, c) => sum + (c.duration_seconds ?? 0), 0);
  const folder = outDir ?? clips[0]?.directory ?? "";
  const canMerge = clips.length >= 2 && name.trim().length > 0 && folder.length > 0;

  const run = useCallback(async () => {
    if (!canMerge) return;
    setBusy(true);
    setPercent(0);
    setResult(null);
    try {
      const target = `${folder}/${name.trim().replace(/\.[^.]*$/, "")}.mp4`;
      const merged = await convertMerge(clips, target, { shape, fit });
      if (!mounted.current) return;
      setResult(merged);
      toast(
        "success",
        merged.how === "copy"
          ? "Joined without re-encoding — no quality lost."
          : "Merged.",
      );
    } catch (e) {
      if (!mounted.current) return;
      const err = toAuthError(e);
      toast("error", downloadMessage(err.code, err.message));
    } finally {
      if (mounted.current) {
        setBusy(false);
        setPercent(0);
      }
    }
  }, [canMerge, clips, folder, name, shape, fit, toast]);

  return (
    <div className="conv">
      <p className="page__lede" style={{ marginTop: 0 }}>
        Drop in the clips you want joined, put them in order, and they become
        one video.
      </p>

      <button
        type="button"
        className={`conv__drop ${dragging ? "conv__drop--over" : ""}`.trim()}
        onClick={() => void chooseClips()}
        disabled={scanning || busy}
      >
        <span className="conv__dropicon">
          <FolderIcon size={20} />
        </span>
        <span className="conv__droptext">
          {scanning
            ? "Reading clips…"
            : dragging
              ? "Drop to add"
              : "Drop video clips here, or click to choose"}
        </span>
        <span className="conv__drophint">
          Pick several at once — they play in the order below, and the arrows
          change it
        </span>
        <span className="conv__dropbtn">
          <span className="btn btn--ghost btn--sm">Choose videos</span>
        </span>
      </button>

      {clips.length > 0 && (
        <section className="conv__panel">
          <header className="conv__tablehead">
            <h2 className="conv__tabletitle">
              Clips<span className="conv__badge">{clips.length}</span>
            </h2>
            <div className="conv__tableactions">
              <span className="conv__selected">
                {formatLength(totalSeconds)} total
              </span>
              <button
                className="btn btn--ghost btn--sm"
                type="button"
                disabled={busy || scanning}
                onClick={() => void chooseClips()}
              >
                Add videos
              </button>
              <button
                className="btn btn--ghost btn--sm"
                type="button"
                disabled={busy}
                onClick={() => {
                  setClips([]);
                  setResult(null);
                }}
              >
                Clear all
              </button>
            </div>
          </header>

          <ul className="mergelist">
            {clips.map((clip, i) => (
              <li key={`${clip.id}-${i}`} className="mergelist__row">
                <span className="clips__index">{i + 1}</span>
                <span className="mergelist__name" title={clip.path}>
                  {clip.file_name}
                </span>
                <span className="mergelist__facts">
                  {formatLength(clip.duration_seconds ?? 0)} ·{" "}
                  {formatResolution(clip.width, clip.height)} ·{" "}
                  {clip.fps ?? "—"} fps
                </span>
                <span className="mergelist__actions">
                  <button
                    className="iconbutton"
                    type="button"
                    disabled={busy || i === 0}
                    onClick={() => move(i, -1)}
                    aria-label="Move up"
                    title="Move up"
                  >
                    <ArrowLeftIcon size={13} className="rot90up" />
                  </button>
                  <button
                    className="iconbutton"
                    type="button"
                    disabled={busy || i === clips.length - 1}
                    onClick={() => move(i, 1)}
                    aria-label="Move down"
                    title="Move down"
                  >
                    <ArrowLeftIcon size={13} className="rot90down" />
                  </button>
                  <button
                    className="iconbutton"
                    type="button"
                    disabled={busy}
                    onClick={() => setClips((prev) => prev.filter((_, n) => n !== i))}
                    aria-label="Remove"
                    title="Remove"
                  >
                    <XIcon size={13} />
                  </button>
                </span>
              </li>
            ))}
          </ul>
        </section>
      )}

      {clips.length > 0 && (
        <section className="conv__card">
          <h3 className="conv__cardtitle">Output</h3>

          {mixedOrientation && (
            <div className="mergeshape">
              <p className="mergeshape__why">
                These clips are a mix of portrait and landscape, so they can't
                all fill the same frame. Choose what the finished video should
                be.
              </p>
              <div className="mergeshape__row">
                <label className="conv__field">
                  <span>Shape</span>
                  <select
                    className="input"
                    value={shape}
                    disabled={busy}
                    onChange={(e) => setShape(e.target.value as MergeShape)}
                  >
                    <option value="first">
                      Match the first clip ({(clips[0]?.width ?? 0) >= (clips[0]?.height ?? 0) ? "landscape" : "portrait"})
                    </option>
                    <option value="landscape">Landscape — 16:9</option>
                    <option value="portrait">Portrait — 9:16</option>
                    <option value="square">Square — 1:1</option>
                  </select>
                </label>
                <label className="conv__field">
                  <span>Clips that don't fit</span>
                  <select
                    className="input"
                    value={fit}
                    disabled={busy}
                    onChange={(e) => setFit(e.target.value as Fit)}
                  >
                    <option value="pad">Black bars — nothing cropped</option>
                    <option value="blur">Blurred backdrop</option>
                    <option value="crop">Crop to fill</option>
                  </select>
                </label>
              </div>
            </div>
          )}

          <div className="split-row">
            <input
              className="input"
              value={name}
              disabled={busy}
              aria-label="File name"
              onChange={(e) => setName(e.target.value)}
            />
            <span className="conv__muted">.mp4</span>
          </div>
          <div className="outdir outdir--inline">
            <div>
              <div className="outdir__label">Save to</div>
              <div className="outdir__path">{folder || "—"}</div>
            </div>
            <div className="outdir__actions">
              <button
                className="btn btn--ghost btn--sm"
                type="button"
                disabled={busy}
                onClick={() => void choose()}
              >
                <FolderIcon size={13} />
                Choose…
              </button>
              {outDir && (
                <button
                  className="btn btn--ghost btn--sm"
                  type="button"
                  disabled={busy}
                  onClick={() => setOutDir(null)}
                >
                  Reset
                </button>
              )}
            </div>
          </div>

          <p className="conv__note">
            {clips.length < 2
              ? "Add at least two clips."
              : fast
                ? "These clips match, so they'll be joined by copying the streams — seconds, with no quality lost."
                : "These clips differ in size, frame rate or codec, so they'll be re-encoded onto one canvas. That takes real time."}
          </p>

          <div className="split-actions">
            <Button
              onClick={() => void run()}
              loading={busy}
              disabled={!canMerge}
              icon={<BoltIcon size={15} />}
            >
              {busy ? "Merging…" : `Merge ${clips.length} clips`}
            </Button>
            {busy && (
              <>
                <span className="split-actions__progress">
                  {Math.round(percent)}%
                </span>
                <button
                  className="btn btn--ghost btn--sm"
                  type="button"
                  onClick={() => void convertCancel()}
                >
                  <StopIcon size={13} />
                  Stop
                </button>
              </>
            )}
          </div>
        </section>
      )}

      {result && (
        <section className="conv__card">
          <div className="split-done">
            <div>
              <div className="split-done__title">
                <CheckIcon size={14} /> {formatLength(result.duration_seconds)} ·{" "}
                {formatBytes(result.size_bytes)}
                {result.how === "copy" ? " · copied, no re-encode" : ""}
              </div>
              <div className="split-done__dir">{result.path}</div>
            </div>
            <Button
              onClick={() => void downloadReveal(result.path)}
              icon={<FolderIcon size={14} />}
            >
              Open folder
            </Button>
          </div>
        </section>
      )}
    </div>
  );
}
