import { useMemo, useState } from "react";
import { Button } from "@/components/ui/Button";
import { useToast } from "@/components/ui/Toast";
import { usePublish } from "@/components/publish/PublishProvider";
import { PlatformMark } from "@/components/publish/PlatformMark";
import { StatusBadge } from "@/components/publish/StatusDot";
import { JobList } from "@/pages/publisher/JobList";
import { ldplayerPickVideo } from "@/lib/ldplayer";
import { ACCOUNT_STATUS_LABEL, publishSubmit } from "@/lib/publish";
import { UploadIcon, SendIcon } from "@/components/ui/icons";

/** Captions longer than this are a paste accident, not a caption. */
const CAPTION_MAX = 2200;

/**
 * Pick a video, write a caption, choose accounts, publish.
 *
 * The selection list shows offline accounts too, greyed but selectable. That
 * is deliberate: the queue boots a stopped instance itself, so refusing to
 * select one would make the user do by hand the exact thing the queue exists
 * to do.
 */
export function PublishPage({ onNavigate }: { onNavigate: (route: "pub-accounts") => void }) {
  const { accounts, jobs, refreshAccounts } = usePublish();
  const toast = useToast();

  const [videoPath, setVideoPath] = useState<string | null>(null);
  const [caption, setCaption] = useState("");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [submitting, setSubmitting] = useState(false);

  const usable = useMemo(
    () => accounts.filter((a) => a.status !== "device_missing"),
    [accounts],
  );

  const fileName = videoPath?.split(/[\\/]/).pop() ?? null;
  const canPublish = Boolean(videoPath) && selected.size > 0 && !submitting;

  const toggle = (id: string) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  const pick = async () => {
    try {
      const path = await ldplayerPickVideo();
      if (path) setVideoPath(path);
    } catch (e) {
      toast("error", `Could not open the file picker: ${message(e)}`);
    }
  };

  const publish = async () => {
    if (!videoPath) return;
    setSubmitting(true);
    try {
      const created = await publishSubmit({
        videoPath,
        caption,
        accountIds: [...selected],
      });
      toast(
        "success",
        `Queued ${created.length} ${created.length === 1 ? "job" : "jobs"}.`,
      );
      // Keep the video and caption: publishing the same clip to another batch
      // of accounts is the next thing people do, and clearing the form would
      // make them redo the picker for no reason.
      setSelected(new Set());
      await refreshAccounts();
    } catch (e) {
      toast("error", `Could not queue: ${message(e)}`);
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="page">
      <header className="page__head">
        <div>
          <h1 className="page__title">Publish</h1>
          <p className="page__lede">
            The video is copied to each emulator over ADB and handed to the app that is
            already signed in there. You tap Post inside the app.
          </p>
        </div>
      </header>

      <section className="pubform">
        <div className="pubform__field">
          <label className="pubform__label">Video</label>
          {videoPath ? (
            <div className="filepick filepick--chosen">
              <div className="filepick__text">
                <div className="filepick__name">{fileName}</div>
                <div className="filepick__path">{videoPath}</div>
              </div>
              <Button variant="ghost" onClick={() => void pick()}>
                Change
              </Button>
            </div>
          ) : (
            <button className="filepick" type="button" onClick={() => void pick()}>
              <UploadIcon size={18} />
              <span>Choose a video from this computer</span>
            </button>
          )}
        </div>

        <div className="pubform__field">
          <label className="pubform__label" htmlFor="caption">
            Caption
          </label>
          <textarea
            id="caption"
            className="textarea"
            rows={4}
            maxLength={CAPTION_MAX}
            placeholder="Check out my new video!"
            value={caption}
            onChange={(e) => setCaption(e.target.value)}
          />
          <div className="pubform__hint">
            <span>
              Facebook receives the caption with the video. Instagram, TikTok and YouTube
              don't accept one from outside their app — paste it in their composer.
            </span>
            <span className="pubform__count">
              {caption.length}/{CAPTION_MAX}
            </span>
          </div>
        </div>

        <div className="pubform__field">
          <div className="pubform__labelrow">
            <label className="pubform__label">Accounts</label>
            {usable.length > 0 && (
              <button
                className="linkbtn"
                type="button"
                onClick={() =>
                  setSelected(
                    selected.size === usable.length
                      ? new Set()
                      : new Set(usable.map((a) => a.id)),
                  )
                }
              >
                {selected.size === usable.length ? "Clear all" : "Select all"}
              </button>
            )}
          </div>

          {usable.length === 0 ? (
            <div className="empty">
              <div className="empty__title">No accounts to publish to</div>
              <div className="empty__text">
                Add the social apps installed on your LDPlayer instances first.
              </div>
              <Button variant="ghost" onClick={() => onNavigate("pub-accounts")}>
                Go to Accounts
              </Button>
            </div>
          ) : (
            <div className="pickgrid">
              {usable.map((account) => {
                const on = selected.has(account.id);
                return (
                  <button
                    key={account.id}
                    type="button"
                    className={`pick ${on ? "pick--on" : ""} ${
                      account.status === "connected" ? "" : "pick--dim"
                    }`.trim()}
                    onClick={() => toggle(account.id)}
                    aria-pressed={on}
                  >
                    <span className="pick__box" aria-hidden>
                      {on ? "✓" : ""}
                    </span>
                    <PlatformMark platform={account.platform} size={26} />
                    <span className="pick__text">
                      <span className="pick__name">{account.name}</span>
                      <span className="pick__meta">
                        {account.device_name ?? account.ldplayer_instance_id}
                      </span>
                    </span>
                    <StatusBadge
                      tone={account.status === "connected" ? "success" : "muted"}
                    >
                      {ACCOUNT_STATUS_LABEL[account.status]}
                    </StatusBadge>
                  </button>
                );
              })}
            </div>
          )}
        </div>

        <footer className="pubform__foot">
          <div className="pubform__summary">
            {selected.size === 0
              ? "Select at least one account"
              : `Publishing to ${selected.size} ${
                  selected.size === 1 ? "account" : "accounts"
                }`}
            {[...selected].some(
              (id) => usable.find((a) => a.id === id)?.status === "device_offline",
            ) && " · stopped emulators will be started for you"}
          </div>
          <Button
            icon={<SendIcon size={15} />}
            disabled={!canPublish}
            loading={submitting}
            onClick={() => void publish()}
          >
            Publish
          </Button>
        </footer>
      </section>

      <section className="section">
        <h2 className="section__title">Queue</h2>
        <JobList jobs={jobs.slice(0, 12)} />
      </section>
    </div>
  );
}

function message(e: unknown): string {
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}
