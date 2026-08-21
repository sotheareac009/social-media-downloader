import { useEffect, useRef, useState } from "react";
import {
  formatBytes,
  formatEta,
  onToolsProgress,
  toolsInstall,
  toolsStatus,
  type ToolState,
} from "@/lib/tools";
import { Button } from "@/components/ui/Button";
import { AlertIcon, CheckIcon, DownloadIcon } from "@/components/ui/icons";

/** The tools we install, in order, with a human label. */
const TOOLS: { id: string; label: string; note: string }[] = [
  { id: "yt-dlp", label: "yt-dlp", note: "Download engine" },
  { id: "ffmpeg", label: "ffmpeg", note: "Merges HD video & audio" },
  { id: "ffprobe", label: "ffprobe", note: "Reads video details" },
];

type Phase = "checking" | "running" | "done" | "error" | "hidden";

/** Byte-level progress for the download currently in flight. */
interface Live {
  tool: string;
  downloaded: number;
  total: number | null;
  speed: number | null;
}

/**
 * First-launch setup. On mount it asks the backend what's missing; if the core
 * tools aren't present (and this platform can install them), it downloads them
 * automatically, showing progress. Everything is dismissible — a user can skip
 * and install manually — and it never appears once the tools are in place.
 */
export function SetupOverlay() {
  const [phase, setPhase] = useState<Phase>("checking");
  const [states, setStates] = useState<Record<string, ToolState>>({});
  const [live, setLive] = useState<Live | null>(null);
  const [error, setError] = useState<string | null>(null);
  const started = useRef(false);

  async function run() {
    setPhase("running");
    setError(null);
    const un = await onToolsProgress((p) => {
      setStates((prev) => ({ ...prev, [p.tool]: p.state }));
      if (p.state === "downloading") {
        setLive({
          tool: p.tool,
          downloaded: p.downloaded_bytes,
          total: p.total_bytes,
          speed: p.bytes_per_sec,
        });
      } else {
        // Clear the bar as soon as this tool stops downloading, so a finished
        // file never leaves a stale percentage on screen.
        setLive((prev) => (prev?.tool === p.tool ? null : prev));
      }
      if (p.state === "failed" && p.error) setError(p.error);
    });
    try {
      const st = await toolsInstall();
      setPhase(st.ready ? "done" : "error");
      if (st.ready) {
        // Tell the rest of the app the tools exist now, so pages that cached
        // "not installed" (e.g. Downloads) re-check without a restart.
        window.dispatchEvent(new CustomEvent("tools-ready"));
        window.setTimeout(() => setPhase("hidden"), 1400);
      }
    } catch (e) {
      setError(messageOf(e));
      setPhase("error");
    } finally {
      un();
    }
  }

  useEffect(() => {
    if (started.current) return;
    started.current = true;
    void (async () => {
      const st = await toolsStatus().catch(() => null);
      // Already set up, or a platform we can't auto-install on: stay out of the way.
      if (!st || st.ready || !st.can_install) {
        setPhase("hidden");
        return;
      }
      void run();
    })();
  }, []);

  if (phase === "hidden" || phase === "checking") return null;

  const running = phase === "running";

  return (
    <div className="setup" role="dialog" aria-modal="true">
      <div className="setup__card rise">
        <div className="setup__icon">
          <DownloadIcon size={22} />
        </div>
        <h2 className="setup__title">
          {phase === "done" ? "You're all set" : "Setting things up"}
        </h2>
        <p className="setup__lede">
          {phase === "done"
            ? "The download tools are installed. You won't see this again."
            : phase === "error"
              ? "Some tools couldn't be installed. You can retry, or install them yourself with Homebrew."
              : "Downloading the tools needed to fetch and process videos. This happens once, and only takes a moment."}
        </p>

        {running && live && <DownloadBar live={live} />}

        <ul className="setup__list">
          {TOOLS.map((t) => {
            const s = states[t.id];
            return (
              <li key={t.id} className="setup__row">
                <span className={`setup__dot setup__dot--${s ?? "idle"}`}>
                  {s === "installed" || s === "skipped" ? (
                    <CheckIcon size={12} />
                  ) : s === "failed" ? (
                    <AlertIcon size={12} />
                  ) : s === "downloading" ? (
                    <span className="setup__spin" />
                  ) : null}
                </span>
                <span className="setup__name">{t.label}</span>
                <span className="setup__note">{t.note}</span>
                <span className="setup__state">
                  {s === "downloading"
                    ? "Downloading…"
                    : s === "installed"
                      ? "Installed"
                      : s === "skipped"
                        ? "Already there"
                        : s === "failed"
                          ? "Failed"
                          : running
                            ? "Waiting"
                            : ""}
                </span>
              </li>
            );
          })}
        </ul>

        {error && phase === "error" && (
          <div className="notice notice--danger" style={{ marginTop: 4 }}>
            <span className="notice__icon"><AlertIcon size={14} /></span>
            <div>{error}</div>
          </div>
        )}

        <div className="setup__actions">
          {phase === "error" && (
            <>
              <Button variant="ghost" onClick={() => setPhase("hidden")}>
                Skip for now
              </Button>
              <Button icon={<DownloadIcon size={15} />} onClick={() => void run()}>
                Retry
              </Button>
            </>
          )}
          {running && (
            <Button variant="ghost" onClick={() => setPhase("hidden")}>
              Hide
            </Button>
          )}
          {phase === "done" && (
            <Button onClick={() => setPhase("hidden")}>Done</Button>
          )}
        </div>
      </div>
    </div>
  );
}

function messageOf(e: unknown): string {
  if (typeof e === "object" && e && "message" in e) {
    return String((e as { message: unknown }).message);
  }
  if (e instanceof Error) return e.message;
  return "Setup failed.";
}

/**
 * Progress for the file currently downloading.
 *
 * ffmpeg is ~163 MB on Windows, and with no feedback at all people reasonably
 * conclude the app has hung. A percentage, a size and a speed make the wait
 * legible; the ETA is coarse on purpose, because a jittery countdown reads as
 * broken.
 */
function DownloadBar({ live }: { live: Live }) {
  const pct =
    live.total && live.total > 0
      ? Math.min(100, Math.round((live.downloaded / live.total) * 100))
      : null;
  const eta = formatEta(live.downloaded, live.total, live.speed);

  return (
    <div className="setupbar">
      <div className="setupbar__head">
        <span className="setupbar__tool">{live.tool}</span>
        <span className="setupbar__pct">{pct !== null ? `${pct}%` : "…"}</span>
      </div>

      <div
        className="setupbar__track"
        role="progressbar"
        aria-valuenow={pct ?? undefined}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-label={`Downloading ${live.tool}`}
      >
        {/* Without Content-Length there is no percentage to show, so the bar
            runs indeterminate rather than sitting at zero. */}
        <div
          className={`setupbar__fill ${pct === null ? "setupbar__fill--unknown" : ""}`.trim()}
          style={pct !== null ? { width: `${pct}%` } : undefined}
        />
      </div>

      <div className="setupbar__meta">
        <span>
          {formatBytes(live.downloaded)}
          {live.total ? ` of ${formatBytes(live.total)}` : ""}
        </span>
        <span>
          {live.speed ? `${formatBytes(live.speed)}/s` : ""}
          {eta ? ` · ${eta}` : ""}
        </span>
      </div>
    </div>
  );
}
