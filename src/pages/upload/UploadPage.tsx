import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  uploadPickFile,
  uploadTargets,
  uploadYoutube,
  uploadYoutubeChannels,
  type Privacy,
  type UploadTarget,
  type YoutubeChannel,
} from "@/lib/upload";
import { Button } from "@/components/ui/Button";
import { useToast } from "@/components/ui/Toast";
import { AlertIcon, CheckIcon, UploadIcon } from "@/components/ui/icons";

/**
 * One screen to publish an asset to a chosen platform. Common fields (file,
 * title, description) plus a platform selector; platform-specific options
 * (YouTube privacy) show conditionally. Only YouTube uploads today; the others
 * appear but say why they're not ready.
 */
export function UploadPage() {
  const toast = useToast();
  const [targets, setTargets] = useState<UploadTarget[] | null>(null);
  const [targetId, setTargetId] = useState("youtube");
  const [file, setFile] = useState<string | null>(null);
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [privacy, setPrivacy] = useState<Privacy>("unlisted");
  const [channel, setChannel] = useState<YoutubeChannel | null | "none">(null);
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

  const pick = useCallback(async () => {
    try {
      const p = await uploadPickFile(accepts as "video" | "photo");
      if (p) setFile(p);
    } catch (e) {
      toast("error", messageOf(e));
    }
  }, [accepts, toast]);

  const publish = useCallback(async () => {
    if (!target?.ready || !file) return;
    setBusy(true);
    try {
      if (target.id === "youtube") {
        const id = await uploadYoutube(file, title, description, privacy);
        toast("success", `Uploaded to YouTube. Video id: ${id}`);
        setFile(null);
        setTitle("");
        setDescription("");
      } else {
        toast("info", "That platform isn't available yet.");
      }
    } catch (e) {
      toast("error", messageOf(e));
    } finally {
      if (mounted.current) setBusy(false);
    }
  }, [target, file, title, description, privacy, toast]);

  const fileName = file?.split("/").pop() ?? null;
  const canPublish = target?.ready === true && !!file && !busy;

  return (
    <div className="page">
      <header className="page__header rise">
        <span className="page__eyebrow">
          <UploadIcon size={12} />
          Upload
        </span>
        <h1 className="page__title">Upload &amp; publish</h1>
        <p className="page__lede">
          Pick a file, add the details, choose where to post it.
        </p>
      </header>

      <div className="fbpost rise" style={{ maxWidth: 620 }}>
        <label className="tg-field__label">Post to</label>
        <div className="up-targets">
          {(targets ?? []).map((t) => (
            <button
              key={t.id}
              type="button"
              className={`up-target ${targetId === t.id ? "up-target--active" : ""} ${
                t.ready ? "" : "up-target--off"
              }`.trim()}
              onClick={() => setTargetId(t.id)}
              title={t.reason ?? undefined}
            >
              <span className="up-target__name">{t.name}</span>
              <span className="up-target__state">
                {t.ready ? (
                  <>
                    <CheckIcon size={11} /> Ready
                  </>
                ) : (
                  "Not ready"
                )}
              </span>
            </button>
          ))}
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

        <label className="tg-field__label" style={{ marginTop: 18 }}>
          {accepts === "video" ? "Video" : "Photo"}
        </label>
        <div className="fbpost__file">
          <button className="btn btn--ghost" type="button" onClick={() => void pick()} disabled={busy}>
            {fileName ? "Change file" : "Choose file"}
          </button>
          {fileName && (
            <span className="fbpost__filename">
              <CheckIcon size={13} /> {fileName}
            </span>
          )}
        </div>

        <label className="tg-field__label" htmlFor="up-title" style={{ marginTop: 16 }}>
          Title
        </label>
        <input
          id="up-title"
          className="tg-field__input"
          placeholder="A title for your upload"
          value={title}
          disabled={busy}
          onChange={(e) => setTitle(e.target.value)}
        />

        <label className="tg-field__label" htmlFor="up-desc" style={{ marginTop: 16 }}>
          Description
        </label>
        <textarea
          id="up-desc"
          className="tg-field__input"
          style={{ minHeight: 90, resize: "vertical" }}
          placeholder="Say more about this…"
          value={description}
          disabled={busy}
          onChange={(e) => setDescription(e.target.value)}
        />

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
            {busy ? "Uploading…" : `Upload to ${target?.name ?? "…"}`}
          </Button>
        </div>
      </div>
    </div>
  );
}

function messageOf(e: unknown): string {
  if (typeof e === "object" && e && "message" in e) {
    return String((e as { message: unknown }).message);
  }
  return "Something went wrong.";
}
