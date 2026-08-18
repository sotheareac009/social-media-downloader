import { CheckIcon, FolderIcon, XIcon } from "@/components/ui/icons";
import type { Destination } from "@/lib/download";

/**
 * Where files land, and how to change it.
 *
 * Choosing and saving are two steps. Browsing to a folder only *proposes* it —
 * nothing changes until Save is pressed — so an accidental pick can't silently
 * redirect every future download, and the saved folder is unambiguously the
 * one the user committed to.
 *
 * Shown inline rather than buried in a Settings page: it's the one setting a
 * person wants to check *before* starting a download, not after.
 */
export function DestinationBar({
  destination,
  pending,
  busy,
  onBrowse,
  onSave,
  onDiscard,
  onReset,
  onOpen,
}: {
  destination: Destination;
  /** A browsed-but-unsaved folder, or null when there's nothing pending. */
  pending: string | null;
  busy: boolean;
  onBrowse: () => void;
  onSave: () => void;
  onDiscard: () => void;
  onReset: () => void;
  onOpen: () => void;
}) {
  if (pending) {
    return (
      <div className="destline destline--pending">
        <span className="destline__icon">
          <FolderIcon size={13} />
        </span>
        <span className="destline__label">Save downloads to</span>
        <span className="destline__path destline__path--static">{pending}</span>
        <div className="destline__actions">
          <button
            className="btn btn--primary btn--sm"
            type="button"
            onClick={onSave}
            disabled={busy}
            aria-busy={busy || undefined}
          >
            {busy ? <span className="btn__spinner" /> : <CheckIcon size={13} />}
            Save
          </button>
          <button
            className="btn btn--ghost btn--sm"
            type="button"
            onClick={onDiscard}
            disabled={busy}
            aria-label="Discard this folder"
            title="Keep the current folder"
          >
            <XIcon size={13} />
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="destline">
      <span className="destline__icon">
        <FolderIcon size={13} />
      </span>
      <span className="destline__label">Saving to</span>
      <button
        className="destline__path"
        type="button"
        onClick={onOpen}
        title="Open this folder"
      >
        {destination.path}
      </button>
      {destination.is_custom && (
        <span className="destline__saved" title="Remembered when you reopen the app">
          <CheckIcon size={11} />
          Saved
        </span>
      )}
      <div className="destline__actions">
        <button
          className="btn btn--ghost btn--sm"
          type="button"
          onClick={onBrowse}
          disabled={busy}
          aria-busy={busy || undefined}
        >
          {busy ? <span className="btn__spinner" /> : null}
          Change…
        </button>
        {destination.is_custom && (
          <button
            className="btn btn--ghost btn--sm"
            type="button"
            onClick={onReset}
            disabled={busy}
            title={`Back to ${destination.default_path}`}
          >
            Reset
          </button>
        )}
      </div>
    </div>
  );
}
