import { useCallback, useEffect, useRef, useState } from "react";
import {
  facebookListPages,
  facebookPickPhoto,
  facebookUploadPhoto,
  type Page,
} from "@/lib/facebook";
import { Button } from "@/components/ui/Button";
import { useToast } from "@/components/ui/Toast";
import { AlertIcon, CheckIcon, UploadIcon } from "@/components/ui/icons";

/**
 * Facebook Page publishing. For now: pick a Page, choose a photo, write a
 * caption, upload. The upload runs entirely in Rust with the Page's own
 * access token — no token reaches this component.
 */
export function FacebookPage() {
  const toast = useToast();
  const [pages, setPages] = useState<Page[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [pageId, setPageId] = useState("");
  const [filePath, setFilePath] = useState<string | null>(null);
  const [caption, setCaption] = useState("");
  const [busy, setBusy] = useState(false);

  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  useEffect(() => {
    void (async () => {
      try {
        const list = await facebookListPages();
        if (!mounted.current) return;
        setPages(list);
        if (list.length > 0) setPageId(list[0].id);
      } catch (e) {
        if (!mounted.current) return;
        setLoadError(messageOf(e));
        setPages([]);
      }
    })();
  }, []);

  const pick = useCallback(async () => {
    try {
      const p = await facebookPickPhoto();
      if (p) setFilePath(p);
    } catch (e) {
      toast("error", messageOf(e));
    }
  }, [toast]);

  const upload = useCallback(async () => {
    if (!pageId || !filePath) return;
    setBusy(true);
    try {
      await facebookUploadPhoto(pageId, filePath, caption);
      toast("success", "Photo posted to your Page.");
      setFilePath(null);
      setCaption("");
    } catch (e) {
      toast("error", messageOf(e));
    } finally {
      if (mounted.current) setBusy(false);
    }
  }, [pageId, filePath, caption, toast]);

  const fileName = filePath?.split("/").pop() ?? null;

  return (
    <div className="page">
      <header className="page__header rise">
        <span
          className="page__eyebrow"
          style={{ color: "#0866FF", borderColor: "color-mix(in srgb, #0866FF 40%, transparent)" }}
        >
          Facebook
        </span>
        <h1 className="page__title">Post a photo to a Page</h1>
        <p className="page__lede">
          Choose a Page you manage, pick a photo, add a caption, and publish.
        </p>
      </header>

      {loadError && (
        <div className="notice notice--danger" style={{ marginBottom: 16 }}>
          <span className="notice__icon"><AlertIcon size={14} /></span>
          <div>{loadError}</div>
        </div>
      )}

      {pages !== null && pages.length === 0 && !loadError && (
        <div className="notice notice--danger" style={{ marginBottom: 16 }}>
          <span className="notice__icon"><AlertIcon size={14} /></span>
          <div>
            No Pages found for this account. You need to manage at least one
            Facebook Page, and the app needs the Page permissions granted.
          </div>
        </div>
      )}

      {pages && pages.length > 0 && (
        <div className="fbpost rise">
          <label className="tg-field__label" htmlFor="fb-page">Page</label>
          <select
            id="fb-page"
            className="quality__select"
            style={{ width: "100%", padding: "10px 12px", fontSize: 14 }}
            value={pageId}
            disabled={busy}
            onChange={(e) => setPageId(e.target.value)}
          >
            {pages.map((p) => (
              <option key={p.id} value={p.id}>{p.name}</option>
            ))}
          </select>

          <label className="tg-field__label" htmlFor="fb-caption" style={{ marginTop: 16 }}>
            Caption
          </label>
          <textarea
            id="fb-caption"
            className="tg-field__input"
            style={{ minHeight: 90, resize: "vertical" }}
            placeholder="Say something about this photo…"
            value={caption}
            disabled={busy}
            onChange={(e) => setCaption(e.target.value)}
          />

          <label className="tg-field__label" style={{ marginTop: 16 }}>Photo</label>
          <div className="fbpost__file">
            <button className="btn btn--ghost" type="button" onClick={() => void pick()} disabled={busy}>
              {fileName ? "Change photo" : "Choose photo"}
            </button>
            {fileName && (
              <span className="fbpost__filename">
                <CheckIcon size={13} /> {fileName}
              </span>
            )}
          </div>

          <div className="fbpost__actions">
            <Button
              loading={busy}
              disabled={!pageId || !filePath}
              icon={<UploadIcon size={15} />}
              onClick={() => void upload()}
            >
              Publish to Page
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}

function messageOf(e: unknown): string {
  if (typeof e === "object" && e && "message" in e) {
    return String((e as { message: unknown }).message);
  }
  return "Something went wrong.";
}
