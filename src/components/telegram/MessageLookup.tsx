import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Button } from "@/components/ui/Button";
import {
  AlertIcon,
  CheckIcon,
  DownloadIcon,
  LinkIcon,
  PlayIcon,
  XIcon,
} from "@/components/ui/icons";
import { useToast } from "@/components/ui/Toast";
import { convertPickOutputDir } from "@/lib/convert";
import { MessageText } from "@/components/telegram/MessageText";
import { MediaLightbox } from "@/components/telegram/MediaLightbox";
import { formatBytes } from "@/lib/download";
import {
  telegramDownloadMedia,
  telegramMediaFileName,
  telegramSaveMedia,
  telegramFetchMessage,
  type TelegramMediaItem,
  type TelegramMessageView,
} from "@/lib/telegram";

/** "3:07" for a clip, "1:02:11" for a long one — the way Telegram writes it. */
function clock(seconds: number): string {
  const s = Math.round(seconds);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const rest = s % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(rest)}` : `${m}:${pad(rest)}`;
}

/**
 * Open a message by its t.me link.
 *
 * Shown as Telegram Desktop shows it — one bubble, the album as a tile grid,
 * the caption underneath — because that is the layout the link came from, and
 * a list of files would lose which caption belongs to which post.
 */
export function MessageLookup() {
  const [link, setLink] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<TelegramMessageView | null>(null);
  const [lightbox, setLightbox] = useState<number | null>(null);
  // Right-click menu position, and the selection it can start. Selecting turns
  // tile clicks into ticks rather than previews, so the two cannot be active
  // by accident — entering selection is always a deliberate act.
  const [menuAt, setMenuAt] = useState<{ x: number; y: number } | null>(null);
  const [selecting, setSelecting] = useState(false);
  const [picked, setPicked] = useState<Set<number>>(new Set());
  const [saving, setSaving] = useState<{ done: number; total: number } | null>(null);
  const toast = useToast();

  // Object URLs from the previous result, revoked when a new one replaces it.
  const previous = useRef<TelegramMessageView | null>(null);

  const search = useCallback(async () => {
    if (link.trim() === "") return;
    setBusy(true);
    setError(null);
    try {
      const found = await telegramFetchMessage(link.trim());
      for (const item of previous.current?.media ?? []) {
        if (item.thumbUrl) URL.revokeObjectURL(item.thumbUrl);
      }
      previous.current = found;
      setMessage(found);
      setSelecting(false);
      setPicked(new Set());
    } catch (e) {
      setMessage(null);
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }, [link]);

  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  // Any click closes the menu, which is what every desktop context menu does.
  useEffect(() => {
    if (!menuAt) return;
    const close = () => setMenuAt(null);
    window.addEventListener("click", close);
    window.addEventListener("contextmenu", close);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("contextmenu", close);
    };
  }, [menuAt]);

  /**
   * Save several items into one folder.
   *
   * The folder is chosen once up front: a save dialog per photo would turn a
   * ten-item album into ten decisions about the same thing. Each file is
   * fetched and written in turn rather than all at once, because ten parallel
   * Telegram downloads is how an account gets rate-limited.
   */
  const download = useCallback(
    async (items: TelegramMediaItem[]) => {
      if (!message || items.length === 0) return;
      let directory: string | null = null;
      try {
        directory = await convertPickOutputDir();
      } catch {
        directory = null;
      }
      if (!directory) return;

      setSaving({ done: 0, total: items.length });
      let written = 0;
      let failed = 0;
      for (const [i, item] of items.entries()) {
        try {
          const { url } = await telegramDownloadMedia(message.chatId, item.messageId);
          const bytes = new Uint8Array(await (await fetch(url)).arrayBuffer());
          // Revoked straight after the copy: an album held in memory at full
          // size is hundreds of megabytes.
          URL.revokeObjectURL(url);
          await telegramSaveMedia(bytes, telegramMediaFileName(item), directory);
          written++;
        } catch {
          // One item failing must not abandon the rest of the album.
          failed++;
        }
        if (!mounted.current) return;
        setSaving({ done: i + 1, total: items.length });
      }

      if (!mounted.current) return;
      setSaving(null);
      toast(
        failed === 0 ? "success" : "error",
        failed === 0
          ? `Saved ${written} file${written === 1 ? "" : "s"} to ${directory}`
          : `Saved ${written}, ${failed} failed.`,
      );
    },
    [message, toast],
  );

  const media = message?.media ?? [];
  // Telegram lays an album out as a mosaic; the shape depends on how many
  // there are, so the count drives the grid rather than a fixed template.
  const albumClass =
    media.length === 1
      ? "tgmsg__album--one"
      : media.length === 2
        ? "tgmsg__album--two"
        : media.length === 3
          ? "tgmsg__album--three"
          : "tgmsg__album--many";

  return (
    <div className="tglookup">
      <form
        className="tglookup__form"
        onSubmit={(e) => {
          e.preventDefault();
          if (!busy) void search();
        }}
      >
        <span className="tglookup__icon">
          <LinkIcon size={15} />
        </span>
        <input
          className="tglookup__input"
          value={link}
          placeholder="https://t.me/channel/123"
          autoComplete="off"
          spellCheck={false}
          disabled={busy}
          onChange={(e) => setLink(e.target.value)}
        />
        <Button type="submit" loading={busy} disabled={link.trim() === ""}>
          Open
        </Button>
      </form>
      <p className="tglookup__hint">
        Paste a message link to see the post. Private channels work too, as long
        as this account is a member.
      </p>

      {error && (
        <div className="notice notice--danger">
          <span className="notice__icon">
            <AlertIcon size={14} />
          </span>
          <div>{error}</div>
        </div>
      )}

      {message && (
        <article
          className="tgmsg"
          onContextMenu={(e) => {
            e.preventDefault();
            setMenuAt({ x: e.clientX, y: e.clientY });
          }}
        >
          <header className="tgmsg__head">
            {message.avatarUrl ? (
              <img className="tgmsg__avatar" src={message.avatarUrl} alt="" />
            ) : (
              <span className="tgmsg__avatar tgmsg__avatar--ph">
                {message.chatTitle.slice(0, 1)}
              </span>
            )}
            <div className="tgmsg__ident">
              <span className="tgmsg__title">{message.chatTitle}</span>
              {message.chatUsername && (
                <span className="tgmsg__handle">@{message.chatUsername}</span>
              )}
            </div>
            {media.length > 0 && !selecting && (
              <button
                className="btn btn--ghost btn--sm"
                type="button"
                disabled={saving !== null}
                onClick={() => void download(media)}
                title="Save every photo and video in this post"
              >
                <DownloadIcon size={13} />
                {saving
                  ? `Saving ${saving.done}/${saving.total}`
                  : media.length > 1
                    ? `Download all ${media.length}`
                    : "Download"}
              </button>
            )}
          </header>

          {selecting && (
            <div className="tgsel">
              <span className="tgsel__count">
                {picked.size} of {media.length} selected
              </span>
              <div className="tgsel__actions">
                <button
                  className="btn btn--ghost btn--sm"
                  type="button"
                  onClick={() =>
                    setPicked(
                      picked.size === media.length
                        ? new Set()
                        : new Set(media.map((_, i) => i)),
                    )
                  }
                >
                  {picked.size === media.length ? "Select none" : "Select all"}
                </button>
                <button
                  className="btn btn--primary btn--sm"
                  type="button"
                  disabled={picked.size === 0 || saving !== null}
                  onClick={() =>
                    void download(media.filter((_, i) => picked.has(i)))
                  }
                >
                  <DownloadIcon size={13} />
                  {saving
                    ? `Saving ${saving.done}/${saving.total}`
                    : `Download selected (${picked.size})`}
                </button>
                <button
                  className="btn btn--ghost btn--sm"
                  type="button"
                  disabled={saving !== null}
                  onClick={() => {
                    setSelecting(false);
                    setPicked(new Set());
                  }}
                  aria-label="Leave selection"
                >
                  <XIcon size={13} />
                </button>
              </div>
            </div>
          )}

          {media.length > 0 && (
            <div className={`tgmsg__album ${albumClass}`}>
              {media.map((item, i) => (
                <button
                  key={`${item.messageId}-${i}`}
                  type="button"
                  className={`tgtile ${item.hasSpoiler ? "tgtile--spoiler" : ""} ${
                    selecting && picked.has(i) ? "tgtile--picked" : ""
                  }`.trim()}
                  // While selecting, a tile ticks instead of opening — a
                  // preview mid-selection loses the ticks behind an overlay.
                  onClick={() =>
                    selecting
                      ? setPicked((prev) => {
                          const next = new Set(prev);
                          if (next.has(i)) next.delete(i);
                          else next.add(i);
                          return next;
                        })
                      : setLightbox(i)
                  }
                  title={selecting ? "Select" : "Open"}
                >
                  {item.thumbUrl ? (
                    <img src={item.thumbUrl} alt="" />
                  ) : (
                    <span className="tgtile__ph" />
                  )}
                  {selecting && (
                    <span
                      className={`tgtile__tick ${picked.has(i) ? "tgtile__tick--on" : ""}`.trim()}
                    >
                      {picked.has(i) && <CheckIcon size={12} />}
                    </span>
                  )}
                  {item.kind === "video" && (
                    <>
                      <span className="tgtile__play">
                        <PlayIcon size={18} />
                      </span>
                      <span className="tgtile__meta">
                        {item.duration ? clock(item.duration) : "video"}
                        {item.sizeBytes ? ` · ${formatBytes(item.sizeBytes)}` : ""}
                      </span>
                    </>
                  )}
                </button>
              ))}
            </div>
          )}

          <MessageText text={message.text} spans={message.spans} />

          <footer className="tgmsg__foot">
            {message.views !== null && (
              <span>{message.views.toLocaleString()} views</span>
            )}
            <span>
              {new Date(message.date * 1000).toLocaleString(undefined, {
                dateStyle: "medium",
                timeStyle: "short",
              })}
            </span>
          </footer>
        </article>
      )}

      {menuAt && message &&
        // Portalled to `body` for the same reason the preview is: the cards
        // this opens from carry a `.rise` animation whose transform persists,
        // and a transformed ancestor becomes the containing block for
        // `position: fixed`. Rendered in place, the menu is positioned
        // relative to the card and lands off-screen — present, but nowhere
        // anyone can see it.
        createPortal(
        <div
          className="tgmenu"
          style={{
            // Clamped so a right-click near the bottom or right edge does not
            // push the menu out of the window.
            left: Math.min(menuAt.x, window.innerWidth - 210),
            top: Math.min(menuAt.y, window.innerHeight - 140),
          }}
          // The window-level listener closes this; a click inside must reach
          // its button first.
          onClick={(e) => e.stopPropagation()}
        >
          <button
            className="tgmenu__item"
            type="button"
            onClick={() => {
              setSelecting(true);
              setPicked(new Set());
              setMenuAt(null);
            }}
          >
            <CheckIcon size={13} />
            Select
          </button>
          <button
            className="tgmenu__item"
            type="button"
            disabled={media.length === 0}
            onClick={() => {
              setMenuAt(null);
              void download(media);
            }}
          >
            <DownloadIcon size={13} />
            Download all ({media.length})
          </button>
          <button
            className="tgmenu__item"
            type="button"
            onClick={() => {
              void navigator.clipboard.writeText(message.link);
              setMenuAt(null);
              toast("info", "Link copied.");
            }}
          >
            <LinkIcon size={13} />
            Copy link
          </button>
        </div>,
        document.body,
      )}

      {message && lightbox !== null && (
        <MediaLightbox
          items={media}
          index={lightbox}
          chatId={message.chatId}
          onIndex={setLightbox}
          onClose={() => setLightbox(null)}
        />
      )}
    </div>
  );
}
