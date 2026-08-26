import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { Button } from "@/components/ui/Button";
import { VideoThumb } from "@/components/convert/VideoThumb";
import { useToast } from "@/components/ui/Toast";
import {
  CheckIcon,
  FolderIcon,
  ScissorsIcon,
  UploadIcon,
  XIcon,
} from "@/components/ui/icons";
import {
  convertPickFile,
  convertPickOutputDir,
  convertProbe,
  convertSplit,
  formatLength,
  looksLikeVideo,
  subscribeToSplitProgress,
  type Clip,
  type SplitProgress,
  type VideoProbe,
} from "@/lib/convert";
import { downloadReveal, formatBytes, downloadMessage } from "@/lib/download";
import { toAuthError } from "@/lib/auth";

/** Where the parts input starts. Two is the smallest split that means anything. */
const DEFAULT_PARTS = 2;

/** Where the length input starts, in whichever unit is showing. */
const DEFAULT_LENGTH = 1;

/** Mirrors MAX_PARTS in Rust — the engine refuses more than this. */
const MAX_PARTS = 200;

type Mode = "count" | "length";
type Unit = "sec" | "min";

/**
 * Names the odd last part, when a video does not divide evenly.
 *
 * A 31-minute video in 30-second clips is 62 of them and a 60-second tail;
 * saying so up front is the difference between a surprise and a plan.
 */
function derivedTail(duration: number, each: number) {
  const whole = Math.floor(duration / each);
  const remainder = duration - whole * each;
  // Under a second is folded into the part before it, so there is no tail to
  // mention; within a whisker of a full part is not worth mentioning either.
  if (remainder < 1 || Math.abs(remainder - each) < 0.5) return null;
  return <> — the last one {formatLength(remainder)}</>;
}

/**
 * Split one long video into equal parts.
 *
 * The whole screen answers one question — "how many pieces?" — and shows what
 * that means in minutes *before* anything runs, because the number of parts is
 * not what anyone actually cares about. Six is only the right answer once you
 * can see it means ten minutes each.
 */
export function SplitTab({ active }: { active: boolean }) {
  const toast = useToast();
  const [probe, setProbe] = useState<VideoProbe | null>(null);
  const [loading, setLoading] = useState(false);
  // Held as strings so a field can be empty while being retyped; a numeric
  // input that snaps back to its default on every keystroke is unusable.
  const [partsText, setPartsText] = useState(String(DEFAULT_PARTS));
  const [lengthText, setLengthText] = useState(String(DEFAULT_LENGTH));
  const [unit, setUnit] = useState<Unit>("min");
  // Which question is being answered: "how many pieces" or "how long each".
  const [mode, setMode] = useState<Mode>("count");
  const [exact, setExact] = useState(false);
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<SplitProgress | null>(null);
  const [clips, setClips] = useState<Clip[] | null>(null);
  const [outputDir, setOutputDir] = useState<string | null>(null);
  // Where the parts should go. null keeps the default: a "<name> (N parts)"
  // folder beside the video, so a split never scatters files into whatever
  // folder the source happened to sit in.
  const [outDir, setOutDir] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);

  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  // "How many parts" — the count is given, the length is derived.
  const parts = Number.parseInt(partsText, 10);
  const countValid = Number.isFinite(parts) && parts >= 2 && parts <= MAX_PARTS;

  // "How long each" — the length is given, the count is derived. This is the
  // shape people actually think in for a platform limit: 30-second clips, not
  // "sixty parts".
  const lengthValue = Number.parseFloat(lengthText);
  const eachSeconds =
    Number.isFinite(lengthValue) && lengthValue > 0
      ? lengthValue * (unit === "min" ? 60 : 1)
      : null;
  // Mirrors plan_by_length in Rust: a leftover too short to watch is folded
  // into the part before it rather than shipped as its own file.
  const derivedCount =
    probe && eachSeconds
      ? (() => {
          const whole = Math.floor(probe.duration_seconds / eachSeconds);
          const remainder = probe.duration_seconds - whole * eachSeconds;
          return remainder >= 1 ? whole + 1 : whole;
        })()
      : null;

  const byCount = mode === "count";
  // What each part will actually be, whichever way it was asked for.
  const each = probe
    ? byCount
      ? countValid
        ? probe.duration_seconds / parts
        : null
      : eachSeconds
    : null;
  const total = byCount ? (countValid ? parts : null) : derivedCount;

  // A part under a second is a glitch, not a clip; so is a "split" into one
  // part, or more parts than the engine will run. Rust refuses all three too —
  // checking here means the button is never enabled for a cut that cannot work.
  const tooShort = each !== null && each < 1;
  const tooMany = total !== null && total > MAX_PARTS;
  const tooFew = total !== null && total < 2;
  const canSplit =
    probe !== null &&
    total !== null &&
    each !== null &&
    !tooShort &&
    !tooMany &&
    !tooFew;

  /** Read a file's length, and clear whatever the last run produced. */
  const load = useCallback(
    async (path: string) => {
      setLoading(true);
      setClips(null);
      setOutputDir(null);
      setProgress(null);
      try {
        const info = await convertProbe(path);
        if (!mounted.current) return;
        setProbe(info);
      } catch (e) {
        if (!mounted.current) return;
        setProbe(null);
        const err = toAuthError(e);
        toast("error", downloadMessage(err.code, err.message));
      } finally {
        if (mounted.current) setLoading(false);
      }
    },
    [toast],
  );

  const choose = useCallback(async () => {
    try {
      const path = await convertPickFile();
      if (path) await load(path);
    } catch (e) {
      toast("error", toAuthError(e).message);
    }
  }, [active, load, toast]);

  const chooseOutDir = useCallback(async () => {
    try {
      const dir = await convertPickOutputDir();
      if (dir) setOutDir(dir);
    } catch (e) {
      toast("error", toAuthError(e).message);
    }
  }, [toast]);

  // Files dropped onto the window. Tauri reports the drop natively — the DOM's
  // own drag events never carry a real path in a webview.
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
      // `enter` carries `paths` just as `drop` does, so anything other than an
      // explicit "drop" must only ever change the highlight - otherwise the
      // video loads while it is still being dragged over the window.
      if (event.payload.type === "enter" || event.payload.type === "over") {
        setDragging(true);
        return;
      }
      if (event.payload.type !== "drop") {
        setDragging(false);
        return;
      }
      setDragging(false);
      // One video at a time: the parts count applies to a single file, and
      // silently splitting only the first of five dropped files would be worse
      // than saying so.
      const paths = event.payload.paths ?? [];
      const video = paths.find(looksLikeVideo);
      if (!video) {
        if (paths.length > 0) {
          toast("error", "That isn't a video file — drop an mp4, mov, mkv or webm.");
        }
        return;
      }
      if (paths.length > 1) {
        toast("info", "Splitting the first video — one file at a time.");
      }
      void load(video);
    });
    return () => {
      void pending.then((unlisten) => unlisten());
    };
  }, [load, toast]);

  // Per-part ticks while a split runs.
  useEffect(() => {
    const pending = subscribeToSplitProgress((p) => {
      if (mounted.current) setProgress(p);
    });
    return () => {
      void pending.then((unlisten) => unlisten());
    };
  }, []);

  const split = useCallback(async () => {
    if (!probe || !canSplit || total === null) return;
    setBusy(true);
    setClips(null);
    setProgress({ index: 1, total, state: "cutting", path: null });
    try {
      // Send whichever question was asked, not the derived answer: Rust owns
      // the arithmetic, so a rounding difference here can never produce a
      // different split than the one the readout promised.
      const result = await convertSplit(
        probe.path,
        byCount ? { parts } : { seconds: eachSeconds as number },
        exact,
        outDir ?? undefined,
      );
      if (!mounted.current) return;
      setClips(result.clips);
      setOutputDir(result.output_dir);
      toast("success", `Cut into ${result.clips.length} parts.`);
    } catch (e) {
      if (!mounted.current) return;
      const err = toAuthError(e);
      toast("error", downloadMessage(err.code, err.message));
    } finally {
      if (mounted.current) {
        setBusy(false);
        setProgress(null);
      }
    }
  }, [probe, parts, byCount, eachSeconds, canSplit, total, exact, outDir, toast]);

  return (
    <div className="conv">
      <p className="page__lede" style={{ marginTop: 0 }}>
        Drop in one long recording, say how many pieces you want, and each part
        is cut to an equal length.
      </p>

      <div
        className={`dropzone rise ${dragging ? "dropzone--over" : ""} ${probe ? "dropzone--loaded" : ""}`.trim()}
      >
        {probe ? (
          <div className="dropzone__file">
            <VideoThumb path={probe.path} className="thumb thumb--lg" />
            <div className="dropzone__meta">
              <div className="dropzone__name">{probe.file_name}</div>
              <div className="dropzone__facts">
                {formatLength(probe.duration_seconds)}
                {probe.width && probe.height
                  ? ` · ${probe.width}×${probe.height}`
                  : ""}
                {` · ${formatBytes(probe.size_bytes)}`}
              </div>
            </div>
            <button
              className="btn btn--ghost btn--sm"
              type="button"
              onClick={() => {
                setProbe(null);
                setClips(null);
                setOutputDir(null);
              }}
              disabled={busy}
            >
              <XIcon size={13} />
              Remove
            </button>
          </div>
        ) : (
          <>
            <span className="dropzone__icon dropzone__icon--big">
              <UploadIcon size={20} />
            </span>
            <div className="dropzone__title">
              {loading ? "Reading the video…" : "Drop a video here"}
            </div>
            <p className="dropzone__hint">
              mp4, mov, mkv, webm and most other formats — or
            </p>
            <Button onClick={() => void choose()} loading={loading}>
              Choose a video
            </Button>
          </>
        )}
      </div>

      {probe && (
        <section className="conv__card rise">
          {/* Radios, not tabs: the two are alternatives, and a tab strip reads
              as two panels that both apply. Only the chosen one is rendered
              below, and the engine refuses a request naming both. */}
          <fieldset className="cutby" disabled={busy}>
            <legend className="cutby__label">Cut by</legend>
            <label className={`cutby__opt ${byCount ? "cutby__opt--on" : ""}`.trim()}>
              <input
                type="radio"
                name="cutby"
                checked={byCount}
                onChange={() => setMode("count")}
              />
              <span>
                <strong>Number of parts</strong>
                <em>Split it into a set number of equal pieces</em>
              </span>
            </label>
            <label className={`cutby__opt ${byCount ? "" : "cutby__opt--on"}`.trim()}>
              <input
                type="radio"
                name="cutby"
                checked={!byCount}
                onChange={() => setMode("length")}
              />
              <span>
                <strong>Length of each part</strong>
                <em>Cut every piece to the same duration</em>
              </span>
            </label>
          </fieldset>

          {byCount ? (
            <div className="split-row">
              <input
                id="parts"
                className="input split-row__input"
                type="number"
                min={2}
                max={MAX_PARTS}
                step={1}
                value={partsText}
                disabled={busy}
                aria-label="Number of parts"
                onChange={(e) => setPartsText(e.target.value)}
              />
              <div className="split-row__result">
                {tooShort ? (
                  <span className="split-row__warn">
                    {parts} parts would be under a second each — this video is{" "}
                    {formatLength(probe.duration_seconds)}.
                  </span>
                ) : each !== null && total !== null ? (
                  <>
                    <strong>{total} videos</strong> of about{" "}
                    <strong>{formatLength(each)}</strong> each
                  </>
                ) : (
                  <span className="split-row__warn">
                    Enter a number between 2 and {MAX_PARTS}.
                  </span>
                )}
              </div>
            </div>
          ) : (
            <div className="split-row">
              <input
                id="length"
                className="input split-row__input"
                type="number"
                min={1}
                step={1}
                value={lengthText}
                disabled={busy}
                aria-label="Length of each part"
                onChange={(e) => setLengthText(e.target.value)}
              />
              <select
                className="input split-row__unit"
                value={unit}
                disabled={busy}
                aria-label="Unit"
                onChange={(e) => setUnit(e.target.value as Unit)}
              >
                <option value="sec">seconds</option>
                <option value="min">minutes</option>
              </select>
              <div className="split-row__result">
                {eachSeconds === null ? (
                  <span className="split-row__warn">
                    Enter how long each part should be.
                  </span>
                ) : tooFew ? (
                  <span className="split-row__warn">
                    That's as long as the video itself (
                    {formatLength(probe.duration_seconds)}) — choose something
                    shorter.
                  </span>
                ) : tooMany ? (
                  <span className="split-row__warn">
                    That would be {total} files — more than the {MAX_PARTS}
                    -part limit. Choose a longer part.
                  </span>
                ) : (
                  <>
                    <strong>{total} videos</strong> of{" "}
                    <strong>{formatLength(eachSeconds)}</strong> each
                    {derivedTail(probe.duration_seconds, eachSeconds)}
                  </>
                )}
              </div>
            </div>
          )}

          <div className="outdir outdir--inline">
            <div>
              <div className="outdir__label">Save the parts to</div>
              <div className="outdir__path" title={outDir ?? undefined}>
                {outDir ??
                  `Beside the video — “${probe.file_name.replace(/\.[^.]+$/, "")} (${total ?? "…"} parts)”`}
              </div>
            </div>
            <div className="outdir__actions">
              <button
                className="btn btn--ghost btn--sm"
                type="button"
                disabled={busy}
                onClick={() => void chooseOutDir()}
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

          <label className="checkline">
            <input
              type="checkbox"
              checked={exact}
              disabled={busy}
              onChange={(e) => setExact(e.target.checked)}
            />
            <span>
              <strong>Cut on the exact second</strong> — re-encodes the video, so
              this takes minutes instead of seconds. Leave it off and each part
              starts at the nearest keyframe, usually within a second or two.
            </span>
          </label>

          <div className="split-actions">
            <Button
              onClick={() => void split()}
              loading={busy}
              disabled={!canSplit}
              icon={<ScissorsIcon size={15} />}
            >
              {busy ? "Cutting…" : `Split into ${canSplit ? total : ""} parts`}
            </Button>
            {busy && progress && (
              <span className="split-actions__progress">
                Part {progress.index} of {progress.total}
                {exact ? " — re-encoding, this takes a while" : ""}
              </span>
            )}
          </div>
        </section>
      )}

      {clips && outputDir && (
        <section className="conv__card rise">
          <div className="split-done">
            <div>
              <div className="split-done__title">
                <CheckIcon size={14} /> {clips.length} parts written
              </div>
              <div className="split-done__dir">{outputDir}</div>
            </div>
            <Button
              onClick={() => void downloadReveal(outputDir)}
              icon={<FolderIcon size={14} />}
            >
              Open folder
            </Button>
          </div>

          <ul className="clips">
            {clips.map((clip) => (
              <li key={clip.path} className="clips__row">
                <span className="clips__index">{clip.index}</span>
                <span className="clips__name">
                  {clip.path.split(/[/\\]/).pop()}
                </span>
                <span className="clips__facts">
                  {formatLength(clip.duration_seconds)} ·{" "}
                  {formatBytes(clip.size_bytes)}
                </span>
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}
