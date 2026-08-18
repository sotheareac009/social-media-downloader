import { TerminalIcon, AlertIcon, CheckIcon } from "@/components/ui/icons";
import type { EngineStatus } from "@/lib/download";

/**
 * The setup gate. yt-dlp is not bundled — it updates far more often than this
 * app ships, and a stale bundled copy would break silently every time a
 * platform changed its page markup. So it's an external dependency, and this
 * panel is what makes that legible instead of mysterious.
 */
export function EngineNotice({
  status,
  onRecheck,
  rechecking,
}: {
  status: EngineStatus;
  onRecheck: () => void;
  rechecking: boolean;
}) {
  if (status.available) {
    return (
      <>
        <div className="engine engine--ok">
          <span className="engine__icon">
            <CheckIcon size={14} />
          </span>
          <div className="engine__body">
            <div className="engine__title">
              Download engine ready
              {status.version && (
                <span className="engine__version">yt-dlp {status.version}</span>
              )}
            </div>
            {status.path && <code className="engine__path">{status.path}</code>}
          </div>
        </div>
        {!status.has_ffmpeg && <FfmpegHint />}
      </>
    );
  }

  return (
    <div className="engine engine--missing">
      <span className="engine__icon">
        <AlertIcon size={14} />
      </span>
      <div className="engine__body">
        <div className="engine__title">yt-dlp isn't installed</div>
        <p className="engine__lede">
          Downloads use <strong>yt-dlp</strong> to read the public page and
          fetch the video file. Install it once, then re-check:
        </p>
        <div className="engine__cmds">
          <Cmd label="macOS" cmd="brew install yt-dlp" />
          <Cmd label="Windows" cmd="winget install yt-dlp.yt-dlp" />
          <Cmd label="Linux / pipx" cmd="pipx install yt-dlp" />
        </div>
        <p className="engine__hint">
          Already installed but not detected? A desktop app doesn't inherit your
          shell's PATH. Set{" "}
          <code>MEDIA_DOWNLOADER_YTDLP=/full/path/to/yt-dlp</code> in your{" "}
          <code>.env</code> and restart.
        </p>
        <button
          className="btn btn--ghost"
          type="button"
          onClick={onRecheck}
          disabled={rechecking}
          aria-busy={rechecking || undefined}
        >
          {rechecking ? <span className="btn__spinner" /> : <TerminalIcon size={14} />}
          Re-check
        </button>
      </div>
    </div>
  );
}

/**
 * YouTube-specific quality warning.
 *
 * Only shown when FFmpeg is absent, and worded around the consequence rather
 * than the dependency: "your YouTube downloads are 360p" is the thing a person
 * cares about. Measured on a real video — 360p progressive versus 1080p
 * merged. Facebook and TikTok serve progressive files, so they're unaffected.
 */
function FfmpegHint() {
  return (
    <div className="engine engine--hint">
      <span className="engine__icon">
        <AlertIcon size={14} />
      </span>
      <div className="engine__body">
        <div className="engine__title">YouTube downloads are capped at 360p</div>
        <p className="engine__lede">
          Above 360p, YouTube serves video and audio as separate streams that
          have to be merged. Install <strong>FFmpeg</strong> to get full quality
          — Facebook and TikTok are unaffected either way.
        </p>
        <div className="engine__cmds">
          <Cmd label="macOS" cmd="brew install ffmpeg" />
          <Cmd label="Windows" cmd="winget install Gyan.FFmpeg" />
          <Cmd label="Linux" cmd="sudo apt install ffmpeg" />
        </div>
        <p className="engine__hint">
          Installed but not found? Set{" "}
          <code>MEDIA_DOWNLOADER_FFMPEG=/full/path/to/ffmpeg</code> in your{" "}
          <code>.env</code> and restart.
        </p>
      </div>
    </div>
  );
}

function Cmd({ label, cmd }: { label: string; cmd: string }) {
  return (
    <div className="engine__cmd">
      <span className="engine__os">{label}</span>
      <code>{cmd}</code>
    </div>
  );
}
