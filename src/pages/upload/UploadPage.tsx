import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import {
  uploadPickFiles,
  uploadVideoThumbnail,
  uploadTargets,
  uploadYoutube,
  uploadYoutubeChannels,
  type Privacy,
  type UploadTarget,
  type YoutubeChannel,
} from "@/lib/upload";
import { Button } from "@/components/ui/Button";
import { useToast } from "@/components/ui/Toast";
import { AlertIcon, CheckIcon, UploadIcon, XIcon } from "@/components/ui/icons";
import { SourceLogo, SOURCE_COLOR, type SourceId } from "@/components/home/SourceLogo";

type ItemStatus = "pending" | "uploading" | "done" | "failed";
interface Item {
  path: string;
  title: string;
  description: string;
  status: ItemStatus;
  error?: string;
}

/** Filename without its extension, used as each video's default title. */
function baseName(path: string): string {
  const name = path.split("/").pop() ?? path;
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(0, dot) : name;
}

export function UploadPage() {
  const toast = useToast();
  const [targets, setTargets] = useState<UploadTarget[] | null>(null);
  const [targetId, setTargetId] = useState("youtube");
  const [items, setItems] = useState<Item[]>([]);
  const [privacy, setPrivacy] = useState<Privacy>("unlisted");
  const [channel, setChannel] = useState<YoutubeChannel | null | "none">(null);
  const [busy, setBusy] = useState(false);
  const [preview, setPreview] = useState<string | null>(null);

  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  useEffect(() => {
    void (async () => {
      const list = await uploadTargets().catch(() => []);
      if (!mounted.current) return;
      setTargets(list);
      const firstReady = list.find((t) => t.ready);
      if (firstReady) setTargetId(firstReady.id);
    })();
  }, []);

  const target = useMemo(
    () => targets?.find((t) => t.id === targetId) ?? null,
    [targets, targetId],
  );
  const accepts = target?.accepts.includes("video") ? "video" : "photo";

  useEffect(() => {
    if (target?.id !== "youtube" || !target.ready) {
      setChannel(null);
      return;
    }
    let alive = true;
    uploadYoutubeChannels()
      .then((cs) => alive && setChannel(cs[0] ?? "none"))
      .catch(() => alive && setChannel(null));
    return () => {
      alive = false;
    };
  }, [target?.id, target?.ready]);

  const addFiles = useCallback(async () => {
    try {
      const paths = await uploadPickFiles(accepts as "video" | "photo");
      if (paths.length === 0) return;
      setItems((prev) => {
        const seen = new Set(prev.map((i) => i.path));
        const added = paths
          .filter((p) => !seen.has(p))
          .map((p): Item => ({
            path: p,
            title: baseName(p),
            description: "",
            status: "pending",
          }));
        return [...prev, ...added];
      });
    } catch (e) {
      toast("error", messageOf(e));
    }
  }, [accepts, toast]);

  const removeItem = useCallback((path: string) => {
    setItems((prev) => prev.filter((i) => i.path !== path));
  }, []);

  const editItem = useCallback((path: string, patch: Partial<Item>) => {
    setItems((prev) => prev.map((i) => (i.path === path ? { ...i, ...patch } : i)));
  }, []);

  const publish = useCallback(async () => {
    if (!target?.ready || items.length === 0) return;
    setBusy(true);
    // Reset any previous outcomes so a re-run reads cleanly.
    setItems((prev) => prev.map((i) => ({ ...i, status: "pending", error: undefined })));

    let ok = 0;
    for (const item of items) {
      setItems((prev) =>
        prev.map((i) => (i.path === item.path ? { ...i, status: "uploading" } : i)),
      );
      try {
        if (target.id === "youtube") {
          const perTitle = item.title.trim() || baseName(item.path);
          await uploadYoutube(item.path, perTitle, item.description, privacy);
        } else {
          throw new Error("That platform isn't available yet.");
        }
        ok += 1;
        setItems((prev) =>
          prev.map((i) => (i.path === item.path ? { ...i, status: "done" } : i)),
        );
      } catch (e) {
        setItems((prev) =>
          prev.map((i) =>
            i.path === item.path ? { ...i, status: "failed", error: messageOf(e) } : i,
          ),
        );
      }
    }

    if (mounted.current) setBusy(false);
    const failed = items.length - ok;
    if (failed === 0) toast("success", `Uploaded ${ok} ${ok === 1 ? "video" : "videos"}.`);
    else toast(ok > 0 ? "info" : "error", `${ok} uploaded, ${failed} failed.`);
  }, [target, items, privacy, toast]);

  const canPublish = target?.ready === true && items.length > 0 && !busy;

  return (
    <div className="page">
      <header className="page__header rise">
        <span className="page__eyebrow">
          <UploadIcon size={12} />
          Upload
        </span>
        <h1 className="page__title">
          Upload &amp; <span className="up-accent">publish</span>
        </h1>
        <p className="page__lede">
          Add one or more files, set the details, choose where to post them.
        </p>
      </header>

      <div className="fbpost rise" style={{ maxWidth: 640 }}>
        <label className="tg-field__label">Post to</label>
        <div className="up-targets">
          {(targets ?? []).map((t) => {
            const brand = SOURCE_COLOR[t.id as SourceId] ?? "var(--accent)";
            return (
              <button
                key={t.id}
                type="button"
                className={`up-target ${targetId === t.id ? "up-target--active" : ""} ${
                  t.ready ? "" : "up-target--off"
                }`.trim()}
                style={{ ["--brand" as string]: brand }}
                onClick={() => setTargetId(t.id)}
                title={t.reason ?? undefined}
              >
                <span className="up-target__edge" />
                <SourceLogo source={t.id as SourceId} />
                <span className="up-target__text">
                  <span className="up-target__name">{t.name}</span>
                  <span
                    className={`up-target__pill ${t.ready ? "up-target__pill--ok" : "up-target__pill--off"}`}
                  >
                    {t.ready ? (
                      <>
                        <CheckIcon size={10} /> Ready
                      </>
                    ) : (
                      "Not ready"
                    )}
                  </span>
                </span>
                {targetId === t.id && (
                  <span className="up-target__check">
                    <CheckIcon size={13} />
                  </span>
                )}
              </button>
            );
          })}
        </div>

        {target && !target.ready && target.reason && (
          <div className="notice notice--danger" style={{ margin: "12px 0 0" }}>
            <span className="notice__icon"><AlertIcon size={14} /></span>
            <div>{target.reason}</div>
          </div>
        )}

        {target?.id === "youtube" && target.ready && channel && channel !== "none" && (
          <div className="up-channel">
            {channel.thumbnail && <img src={channel.thumbnail} alt="" />}
            <span>
              Uploading to <strong>{channel.title}</strong>
            </span>
          </div>
        )}
        {target?.id === "youtube" && channel === "none" && (
          <div className="notice notice--danger" style={{ margin: "12px 0 0" }}>
            <span className="notice__icon"><AlertIcon size={14} /></span>
            <div>This Google account has no YouTube channel yet. Create one on youtube.com first.</div>
          </div>
        )}

        {/* Files */}
        <div className="up-files-head">
          <label className="tg-field__label" style={{ margin: 0 }}>
            {accepts === "video" ? "Videos" : "Photos"}
            {items.length > 0 && <span className="up-count"> ({items.length})</span>}
          </label>
          <div className="up-files-actions">
            {items.length > 0 && !busy && (
              <button className="btn btn--ghost btn--sm" type="button" onClick={() => setItems([])}>
                Clear all
              </button>
            )}
            <button className="btn btn--ghost btn--sm" type="button" onClick={() => void addFiles()} disabled={busy}>
              Add files
            </button>
          </div>
        </div>

        {items.length === 0 ? (
          <p className="up-empty">No files added yet. Click “Add files” to choose one or more.</p>
        ) : (
          <ul className="up-list">
            {items.map((item) => (
              <li key={item.path} className={`up-item up-item--${item.status}`}>
                <button
                  type="button"
                  className="up-item__thumb"
                  onClick={() => setPreview(item.path)}
                  title="Preview"
                  aria-label="Preview"
                >
                  <MediaThumb path={item.path} kind={accepts as "video" | "photo"} />
                  <span className="up-item__play">▶</span>
                </button>
                <div className="up-item__body">
                  <div className="up-item__row">
                    <span className="up-item__name" title={item.path.split("/").pop()}>
                      {item.path.split("/").pop()}
                    </span>
                    {!busy && (
                      <button
                        className="up-item__remove"
                        type="button"
                        onClick={() => removeItem(item.path)}
                        aria-label="Remove"
                        title="Remove"
                      >
                        <XIcon size={14} />
                      </button>
                    )}
                  </div>
                  <input
                    className="tg-field__input up-item__field"
                    placeholder="Title"
                    value={item.title}
                    disabled={busy}
                    onChange={(e) => editItem(item.path, { title: e.target.value })}
                  />
                  <textarea
                    className="tg-field__input up-item__field"
                    style={{ minHeight: 52, resize: "vertical" }}
                    placeholder="Description (optional)"
                    value={item.description}
                    disabled={busy}
                    onChange={(e) => editItem(item.path, { description: e.target.value })}
                  />
                  <div className="up-item__status">
                    {item.status === "uploading" && "Uploading…"}
                    {item.status === "done" && (
                      <span className="up-item__ok"><CheckIcon size={11} /> Uploaded</span>
                    )}
                    {item.status === "failed" && (
                      <span className="up-item__bad">{item.error ?? "Failed"}</span>
                    )}
                  </div>
                </div>
              </li>
            ))}
          </ul>
        )}

        {target?.id === "youtube" && (
          <>
            <label className="tg-field__label" htmlFor="up-privacy" style={{ marginTop: 16 }}>
              Visibility
            </label>
            <select
              id="up-privacy"
              className="quality__select"
              style={{ width: "100%", padding: "10px 12px", fontSize: 14 }}
              value={privacy}
              disabled={busy}
              onChange={(e) => setPrivacy(e.target.value as Privacy)}
            >
              <option value="private">Private</option>
              <option value="unlisted">Unlisted</option>
              <option value="public">Public</option>
            </select>
          </>
        )}

        <div className="fbpost__actions">
          <Button
            loading={busy}
            disabled={!canPublish}
            icon={<UploadIcon size={15} />}
            onClick={() => void publish()}
          >
            {busy
              ? "Uploading…"
              : `Upload ${items.length > 1 ? `${items.length} to` : "to"} ${target?.name ?? "…"}`}
          </Button>
        </div>
      </div>

      {preview && (
        <div
          className="up-modal"
          role="dialog"
          aria-modal="true"
          onClick={() => setPreview(null)}
        >
          <div className="up-modal__inner" onClick={(e) => e.stopPropagation()}>
            <button
              className="up-modal__close"
              type="button"
              onClick={() => setPreview(null)}
              aria-label="Close preview"
            >
              <XIcon size={18} />
            </button>
            {accepts === "video" ? (
              <video src={convertFileSrc(preview)} controls autoPlay playsInline />
            ) : (
              <img src={convertFileSrc(preview)} alt="Preview" />
            )}
          </div>
        </div>
      )}
    </div>
  );
}

/** Thumbnail for a list item: a real poster frame for videos, the image itself
 *  for photos. Falls back to a placeholder while (or if) no frame is available. */
function MediaThumb({ path, kind }: { path: string; kind: "video" | "photo" }) {
  const [poster, setPoster] = useState<string | null>(null);
  const [tried, setTried] = useState(false);

  useEffect(() => {
    if (kind !== "video") return;
    let alive = true;
    uploadVideoThumbnail(path)
      .then((p) => alive && setPoster(p))
      .catch(() => {})
      .finally(() => alive && setTried(true));
    return () => {
      alive = false;
    };
  }, [path, kind]);

  if (kind === "photo") {
    return <img src={convertFileSrc(path)} alt="" />;
  }
  if (poster) return <img src={poster} alt="" />;
  return (
    <div className={`up-thumb-fallback ${tried ? "" : "up-thumb-fallback--load"}`.trim()}>
      <UploadIcon size={16} />
    </div>
  );
}

function messageOf(e: unknown): string {
  if (typeof e === "object" && e && "message" in e) {
    return String((e as { message: unknown }).message);
  }
  if (e instanceof Error) return e.message;
  return "Something went wrong.";
}
