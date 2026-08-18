import { Button } from "@/components/ui/Button";
import { DownloadIcon, UsersIcon, XIcon } from "@/components/ui/icons";
import { formatDuration, type ProfileListing } from "@/lib/download";

/**
 * A profile awaiting confirmation.
 *
 * Pasting one profile link can mean a hundred downloads, so the count is shown
 * and the user decides. Queueing them silently would be the kind of surprise
 * that fills a disk.
 */
export function ProfileCard({
  listing,
  busy,
  onConfirm,
  onDismiss,
}: {
  listing: ProfileListing;
  busy: boolean;
  onConfirm: () => void;
  onDismiss: () => void;
}) {
  const preview = listing.entries.slice(0, 3);
  const rest = listing.count - preview.length;

  return (
    <article className="profile">
      <div className="profile__head">
        <span className="profile__avatar">
          <UsersIcon size={16} />
        </span>
        <div className="profile__ident">
          <div className="profile__name">@{listing.uploader}</div>
          <div className="profile__count">
            <strong>{listing.count}</strong>{" "}
            {listing.count === 1 ? "video" : "videos"} found
          </div>
        </div>
        <div className="profile__actions">
          <Button
            loading={busy}
            onClick={onConfirm}
            icon={<DownloadIcon size={14} />}
          >
            Download all {listing.count}
          </Button>
          <button
            className="btn btn--ghost btn--sm"
            type="button"
            onClick={onDismiss}
            disabled={busy}
            aria-label="Dismiss"
            title="Dismiss"
          >
            <XIcon size={13} />
          </button>
        </div>
      </div>

      <ul className="profile__preview">
        {preview.map((e) => (
          <li key={e.id || e.url}>
            <span className="profile__ptitle">{e.title ?? e.url}</span>
            {e.duration_seconds !== null && (
              <span className="profile__pdur">
                {formatDuration(e.duration_seconds)}
              </span>
            )}
          </li>
        ))}
        {rest > 0 && <li className="profile__more">and {rest} more…</li>}
      </ul>

      {listing.count > 40 && (
        <p className="profile__warn">
          That's a lot of files. Two download at a time, so this will take a
          while — you can cancel individual videos at any point.
        </p>
      )}
    </article>
  );
}
