import { useMemo, useState } from "react";
import { Button } from "@/components/ui/Button";
import { useToast } from "@/components/ui/Toast";
import { usePublish } from "@/components/publish/PublishProvider";
import { PlatformMark } from "@/components/publish/PlatformMark";
import { StatusBadge } from "@/components/publish/StatusDot";
import { JobList } from "@/pages/publisher/JobList";
import { convertFileSrc } from "@tauri-apps/api/core";
import { ldplayerPickMedia, mediaKindOf, type MediaCollection } from "@/lib/ldplayer";
import { ACCOUNT_STATUS_LABEL, publishSubmit, type PostMode } from "@/lib/publish";
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

  const [paths, setPaths] = useState<string[]>([]);
  const [mode, setMode] = useState<PostMode>("album");
  const [caption, setCaption] = useState("");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [submitting, setSubmitting] = useState(false);

  const usable = useMemo(
    () => accounts.filter((a) => a.status !== "device_missing"),
    [accounts],
  );

  const canPublish = paths.length > 0 && selected.size > 0 && !submitting;

  // What Publish will actually create. Shown before the button, because
  // "6 posts" and "2 posts" are very different things to do to a real account.
  const jobCount =
    mode === "album" ? selected.size : selected.size * paths.length;

  const toggle = (id: string) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  const pick = async () => {
    try {
      const picked = await ldplayerPickMedia();
      // Append rather than replace: choosing an album usually means several
      // trips to the picker, and replacing would silently discard the first.
      if (picked.length > 0) {
        setPaths((prev) => [...prev, ...picked.filter((p) => !prev.includes(p))]);
      }
    } catch (e) {
      toast("error", `Could not open the file picker: ${message(e)}`);
    }
  };

  const publish = async () => {
    if (paths.length === 0) return;
    setSubmitting(true);
    try {
      const created = await publishSubmit({
        paths,
        caption,
        accountIds: [...selected],
        mode,
      });
      toast(
        "success",
        `Queued ${created.length} ${created.length === 1 ? "job" : "jobs"}.`,
      );
      // Keep the files and caption: publishing the same set to another batch
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
            The video or photo is copied to each emulator over ADB and handed to the app
            that is already signed in there. You tap Post inside the app.
          </p>
        </div>
      </header>

      <section className="pubform">
        <div className="pubform__field">
          <div className="pubform__labelrow">
            <label className="pubform__label">
              Videos and photos
              {paths.length > 0 && (
                <span className="pubform__badge">{paths.length}</span>
              )}
            </label>
            {paths.length > 0 && (
              <button className="linkbtn" type="button" onClick={() => setPaths([])}>
                Clear all
              </button>
            )}
          </div>

          {paths.length === 0 ? (
            <button className="filepick" type="button" onClick={() => void pick()}>
              <UploadIcon size={18} />
              <span>Choose videos or photos from this computer</span>
            </button>
          ) : (
            <>
              <div className="assetlist">
                {paths.map((path, i) => (
                  <AssetCard
                    key={path}
                    path={path}
                    index={i}
                    total={paths.length}
                    // Order is only meaningful for an album, so the reorder
                    // controls disappear when it cannot change anything.
                    ordered={mode === "album" && paths.length > 1}
                    onMove={(to) => setPaths((prev) => move(prev, i, to))}
                    onRemove={() => setPaths((prev) => prev.filter((p) => p !== path))}
                  />
                ))}
              </div>
              <button className="addmore" type="button" onClick={() => void pick()}>
                + Add more
              </button>
            </>
          )}
        </div>

        {paths.length > 1 && (
          <div className="pubform__field">
            <label className="pubform__label">Post as</label>
            <div className="modepick">
              <ModeOption
                mode="album"
                current={mode}
                onPick={setMode}
                title="One album post"
                detail={`All ${paths.length} files in a single post, per account.`}
              />
              <ModeOption
                mode="single"
                current={mode}
                onPick={setMode}
                title="Separate posts"
                detail={`Each file posted on its own — ${paths.length} posts per account.`}
              />
            </div>
            {mode === "album" && (
              <div className="pubform__hint">
                <span>
                  Android has no way to attach several files to an app from outside it,
                  so the app opens with all {paths.length} in its gallery and you tick
                  them in the order shown above.
                </span>
              </div>
            )}
          </div>
        )}

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
              Facebook receives the caption with the file. Instagram, TikTok and YouTube
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
            {paths.length === 0
              ? "Choose at least one video or photo"
              : selected.size === 0
                ? "Select at least one account"
                : `${jobCount} ${jobCount === 1 ? "post" : "posts"} across ${
                    selected.size
                  } ${selected.size === 1 ? "account" : "accounts"}`}
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

/**
 * One chosen asset: a thumbnail, its name, and controls.
 *
 * Showing the file rather than naming it catches the mistake people actually
 * make — last week's export, or the wrong take of the same clip. It costs
 * nothing: the file is already local, served through Tauri's asset protocol.
 *
 * Video is deliberately not autoplayed. Six thumbnails all starting at once,
 * beside a running publish, would be chaos.
 */
function AssetCard({
  path,
  index,
  total,
  ordered,
  onMove,
  onRemove,
}: {
  path: string;
  index: number;
  total: number;
  ordered: boolean;
  onMove: (to: number) => void;
  onRemove: () => void;
}) {
  const src = convertFileSrc(path);
  const fileName = path.split(/[\\/]/).pop() ?? path;
  const kind: MediaCollection = mediaKindOf(path);

  return (
    <div className="asset">
      <div className="asset__frame">
        {kind === "video" ? (
          <video className="asset__media" src={src} controls preload="metadata" />
        ) : (
          <img className="asset__media" src={src} alt={fileName} />
        )}
        {ordered && <span className="asset__order">{index + 1}</span>}
      </div>

      <div className="asset__name" title={path}>
        {fileName}
      </div>
      <div className="asset__kind">{kind === "video" ? "Video" : "Photo"}</div>

      <div className="asset__actions">
        {ordered && (
          <>
            <button
              className="iconbtn"
              type="button"
              disabled={index === 0}
              onClick={() => onMove(index - 1)}
              aria-label={`Move ${fileName} earlier`}
              title="Move earlier"
            >
              ↑
            </button>
            <button
              className="iconbtn"
              type="button"
              disabled={index === total - 1}
              onClick={() => onMove(index + 1)}
              aria-label={`Move ${fileName} later`}
              title="Move later"
            >
              ↓
            </button>
          </>
        )}
        <button
          className="iconbtn"
          type="button"
          onClick={onRemove}
          aria-label={`Remove ${fileName}`}
          title="Remove"
        >
          ✕
        </button>
      </div>
    </div>
  );
}

function ModeOption({
  mode,
  current,
  onPick,
  title,
  detail,
}: {
  mode: PostMode;
  current: PostMode;
  onPick: (m: PostMode) => void;
  title: string;
  detail: string;
}) {
  const on = current === mode;
  return (
    <button
      className={`modeopt ${on ? "modeopt--on" : ""}`.trim()}
      type="button"
      onClick={() => onPick(mode)}
      aria-pressed={on}
    >
      <span className="modeopt__radio" aria-hidden>
        {on ? "●" : ""}
      </span>
      <span className="modeopt__text">
        <span className="modeopt__title">{title}</span>
        <span className="modeopt__detail">{detail}</span>
      </span>
    </button>
  );
}

/** Move one item within a list, returning a new array. */
function move<T>(list: T[], from: number, to: number): T[] {
  if (to < 0 || to >= list.length) return list;
  const next = [...list];
  const [item] = next.splice(from, 1);
  next.splice(to, 0, item);
  return next;
}

function message(e: unknown): string {
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}
