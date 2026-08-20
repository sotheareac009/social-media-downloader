import { AlertIcon, GlobeIcon } from "@/components/ui/icons";

/**
 * Shown on the Downloads page whenever connectivity probes are failing.
 *
 * Deliberately a full panel rather than a disabled button or a toast: a
 * disabled control with no explanation reads as a bug, and a toast is gone
 * before the user works out why nothing happened. It mirrors the engine-missing
 * panel so "something is stopping downloads" always looks the same.
 */
export function OfflineNotice({
  onRecheck,
  checking,
}: {
  onRecheck: () => void;
  checking: boolean;
}) {
  return (
    <div className="engine engine--missing">
      <span className="engine__icon">
        <AlertIcon size={14} />
      </span>
      <div className="engine__body">
        <div className="engine__title">You're offline</div>
        <p className="engine__lede">
          Downloading needs an internet connection — the engine has to open the
          public page and fetch the video from the platform's own servers.
          Nothing can be fetched until the connection is back.
        </p>
        <p className="engine__hint">
          Files you've already downloaded are on your computer and still play
          normally. Anything that was mid-download will have stopped and can be
          started again once you're reconnected.
        </p>
        <button
          className="btn btn--ghost"
          type="button"
          onClick={onRecheck}
          disabled={checking}
          aria-busy={checking || undefined}
        >
          {checking ? <span className="btn__spinner" /> : <GlobeIcon size={14} />}
          {checking ? "Checking…" : "Check again"}
        </button>
      </div>
    </div>
  );
}
