import { Button } from "@/components/ui/Button";
import { DownloadIcon, ListIcon, UsersIcon, XIcon } from "@/components/ui/icons";
import { formatDuration, type ProfileListing } from "@/lib/download";

/**
 * A profile or playlist awaiting confirmation.
 *
 * Pasting one such link can mean a hundred downloads, so the count is shown
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
  // A playlist has a title, not a handle: "@Best of 2024" reads as a broken
  // username rather than the name of the list that was found.
  const isPlaylist = listing.kind === "playlist";

  return (
    <article className="profile">
      <div className="profile__head">
        <span className="profile__avatar">
          {isPlaylist ? <ListIcon size={16} /> : <UsersIcon size={16} />}
        </span>
        <div className="profile__ident">
          <div className="profile__name">
            {isPlaylist ? listing.uploader : `@${listing.uploader}`}
          </div>
          <div className="profile__count">
            <strong>{listing.count}</strong>{" "}
            {listing.count === 1 ? "video" : "videos"}{" "}
            {isPlaylist ? "in this playlist" : "found"}
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
