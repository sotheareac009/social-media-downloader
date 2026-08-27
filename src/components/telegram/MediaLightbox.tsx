import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { ArrowLeftIcon, ChevronRightIcon, DownloadIcon, XIcon } from "@/components/ui/icons";
import { formatBytes } from "@/lib/download";
import { useToast } from "@/components/ui/Toast";
import { telegramDownloadMedia, telegramSaveMedia, type TelegramMediaItem } from "@/lib/telegram";

/**
 * Full-screen media preview, in the shape Telegram Desktop uses: the picture
 * centred on a dark ground, arrows either side of an album, Escape or a click
 * on the backdrop to leave.
 *
 * RENDERED THROUGH A PORTAL, and it has to be. `position: fixed` resolves
 * against the nearest transformed ancestor rather than the viewport, and the
 * cards this opens from carry a `.rise` animation whose transform persists —
 * so an overlay rendered in place is trapped inside the card it came from,
 * covering a corner of the page instead of the app. A portal to `body` leaves
 * every one of those ancestors behind.
 *
 * The thumbnail is shown immediately and the full file downloads behind it, so
 * opening a 40 MB video is not a blank rectangle for ten seconds. Every object
 * URL is revoked on close — a few videos left in memory is hundreds of
 * megabytes the app never gets back.
 */
export function MediaLightbox({
  items,
  index,
  chatId,
  onIndex,
  onClose,
}: {
  items: TelegramMediaItem[];
  index: number;
  chatId: string;
  onIndex: (next: number) => void;
  onClose: () => void;
}) {
  const toast = useToast();
  const item = items[index];
  const [url, setUrl] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [percent, setPercent] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Keyed by message id, so paging back and forth downloads each file once.
  const cache = useRef(new Map<number, string>());
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  useEffect(() => {
    const held = cache.current;
    return () => {
      for (const objectUrl of held.values()) URL.revokeObjectURL(objectUrl);
      held.clear();
    };
  }, []);

  useEffect(() => {
    if (!item) return;
    const cached = cache.current.get(item.messageId);
    if (cached) {
      setUrl(cached);
      setPercent(null);
      return;
    }
    setUrl(null);
    setPercent(0);
    setError(null);
    let alive = true;
    void telegramDownloadMedia(chatId, item.messageId, (received, total) => {
      if (alive && total) setPercent((received / total) * 100);
    })
      .then(({ url: objectUrl }) => {
        if (!alive) {
          URL.revokeObjectURL(objectUrl);
          return;
        }
        cache.current.set(item.messageId, objectUrl);
        setUrl(objectUrl);
        setPercent(null);
      })
      .catch((e) => {
        if (alive) {
          setError(e instanceof Error ? e.message : String(e));
          setPercent(null);
        }
      });
    return () => {
      alive = false;
    };
  }, [item, chatId]);

  const step = useCallback(
    (by: number) => {
      const next = index + by;
      if (next >= 0 && next < items.length) onIndex(next);
    },
    [index, items.length, onIndex],
  );

  // The page behind must not scroll under the overlay, which is what happens
  // when a trackpad gesture lands on the backdrop.
  useEffect(() => {
    const previous = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => {
      document.body.style.overflow = previous;
    };
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
      if (e.key === "ArrowLeft") step(-1);
      if (e.key === "ArrowRight") step(1);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose, step]);

  if (!item) return null;

  const save = async () => {
    if (!url || saving) return;
    setSaving(true);
    try {
      // The file is already in memory as the object URL backing the preview,
      // so saving re-reads that rather than downloading a second time.
      const bytes = new Uint8Array(await (await fetch(url)).arrayBuffer());
      const name =
        item.fileName ??
        `telegram-${item.messageId}.${item.kind === "video" ? "mp4" : "jpg"}`;
      const path = await telegramSaveMedia(bytes, name);
      if (path) toast("success", `Saved to ${path}`);
    } catch (e) {
      toast("error", `Couldn't save: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      if (mounted.current) setSaving(false);
    }
  };

  return createPortal(
    // Clicking the ground closes; clicking the picture must not, which is what
    // the stopPropagation below is for.
    <div className="lightbox" onClick={onClose} role="dialog" aria-modal="true">
      <header className="lightbox__bar" onClick={(e) => e.stopPropagation()}>
        <span className="lightbox__count">
          {items.length > 1 ? `${index + 1} of ${items.length}` : item.fileName ?? "Media"}
          {item.sizeBytes ? ` · ${formatBytes(item.sizeBytes)}` : ""}
        </span>
        <div className="lightbox__actions">
          <button
            className="lightbox__btn"
            type="button"
            onClick={() => void save()}
            disabled={!url || saving}
            title={saving ? "Saving…" : "Save to disk"}
          >
            <DownloadIcon size={16} />
          </button>
          <button className="lightbox__btn" type="button" onClick={onClose} title="Close">
            <XIcon size={16} />
          </button>
        </div>
      </header>

      {items.length > 1 && index > 0 && (
        <button
          className="lightbox__nav lightbox__nav--prev"
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            step(-1);
          }}
          aria-label="Previous"
        >
          <ArrowLeftIcon size={20} />
        </button>
      )}

      <div className="lightbox__stage" onClick={(e) => e.stopPropagation()}>
        {error ? (
          <div className="lightbox__error">{error}</div>
        ) : url && item.kind === "video" ? (
          <video className="lightbox__media" src={url} controls autoPlay />
        ) : url ? (
          <img className="lightbox__media" src={url} alt="" />
        ) : (
          <div className="lightbox__loading">
            {/* The thumbnail stands in while the real file arrives. */}
            {item.thumbUrl && (
              <img className="lightbox__media lightbox__media--blur" src={item.thumbUrl} alt="" />
            )}
            <span className="lightbox__progress">
              {percent === null ? "Loading…" : `${Math.round(percent)}%`}
            </span>
          </div>
        )}
      </div>

      {items.length > 1 && index < items.length - 1 && (
        <button
          className="lightbox__nav lightbox__nav--next"
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            step(1);
          }}
          aria-label="Next"
        >
          <ChevronRightIcon size={20} />
        </button>
      )}
    </div>,
    document.body,
  );
}
