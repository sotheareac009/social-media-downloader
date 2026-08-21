import { useMemo, useState } from "react";
import { Button } from "@/components/ui/Button";
import { useToast } from "@/components/ui/Toast";
import { usePublish } from "@/components/publish/PublishProvider";
import { PlatformMark } from "@/components/publish/PlatformMark";
import { StatusBadge } from "@/components/publish/StatusDot";
import { JobList } from "@/pages/publisher/JobList";
import { convertFileSrc } from "@tauri-apps/api/core";
import { ldplayerPickMedia, mediaKindOf, type MediaCollection } from "@/lib/ldplayer";
import {
  ACCOUNT_STATUS_LABEL,
  publishSubmit,
  type PostMode,
  type VideoFormat,
} from "@/lib/publish";
import { UploadIcon, SendIcon } from "@/components/ui/icons";

/** Captions longer than this are a paste accident, not a caption. */
/// How many targets the picker shows before you have to search for one.
const TARGETS_SHOWN = 5;

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
  const [videoFormat, setVideoFormat] = useState<VideoFormat>("post");
  const [caption, setCaption] = useState("");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [query, setQuery] = useState("");
  const [submitting, setSubmitting] = useState(false);

  // `available` keeps accounts for a switched-off platform out of the target
  // list. They stay on the Accounts page, where they can be seen and removed
  // — hiding a row everywhere would leave something in the database that
  // nobody could get at.
  const usable = useMemo(
    () => accounts.filter((a) => a.status !== "device_missing" && a.available),
    [accounts],
  );

  // What you actually pick is an identity to post AS, not an app to post
  // through. An account with Pages contributes one target per Page and does
  // not offer the profile: posting to Pages is what these accounts are for,
  // and a profile sitting among them is the row people click by mistake.
  //
  // An account with no Pages yet still offers itself, so posting keeps
  // working before any Page has been added.
  const targets = useMemo(
    () =>
      usable.flatMap((account) =>
        account.pages.length > 0
          ? account.pages.map((page) => ({
              key: `${account.id}::${page.name}`,
              account,
              page: page.name as string | null,
              label: page.name,
            }))
          : [{ key: account.id, account, page: null as string | null, label: account.name }],
      ),
    [usable],
  );

  // Search results, or null when nothing has been typed. Matching the owning
  // account and emulator as well as the Page name is what makes "everything on
  // instance 2" a findable set rather than a memory exercise.
  const matches = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return null;
    return targets.filter(
      (t) =>
        t.label.toLowerCase().includes(q) ||
        t.account.name.toLowerCase().includes(q) ||
        (t.account.device_name ?? "").toLowerCase().includes(q),
    );
  }, [targets, query]);

  // A handful by default, the rest by searching: a person with forty Pages
  // scrolls past thirty-nine to reach the one they want, and a wall of
  // checkboxes next to a Publish button is its own hazard.
  //
  // Anything already picked stays on screen whatever the filter says —
  // a selection you cannot see is one you cannot undo, and it still posts.
  const visible = useMemo(() => {
    const base = new Set((matches ?? targets.slice(0, TARGETS_SHOWN)).map((t) => t.key));
    return targets.filter((t) => base.has(t.key) || selected.has(t.key));
  }, [matches, targets, selected]);

  const hidden = targets.length - visible.length;

  // Only videos get the Reel/Post question, so only they show it. Matched on
  // extension, the same way the device layer decides which MediaStore
  // collection a file belongs in.
  const hasVideo = useMemo(
    () => paths.some((p) => /\.(mp4|mov|mkv|webm|avi|m4v)$/i.test(p)),
    [paths],
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
        targets: targets
          .filter((t) => selected.has(t.key))
          .map((t) => ({ account_id: t.account.id, page: t.page })),
        mode,
        videoFormat,
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

        {hasVideo && (
          <div className="pubform__field">
            <label className="pubform__label">Video goes out as</label>
            <div className="modepick">
              <FormatOption
                format="post"
                current={videoFormat}
                onPick={setVideoFormat}
                title="Feed post"
                detail="An ordinary post on the timeline."
              />
              <FormatOption
                format="reel"
                current={videoFormat}
                onPick={setVideoFormat}
                title="Reel"
                detail="Facebook's short-form video surface, with its own editor."
              />
            </div>
            <div className="pubform__hint">
              <span>
                Facebook asks which of these a shared video should become. This answers
                it for you — on the versions that don't ask, the app decides and this
                has no effect.
              </span>
            </div>
          </div>
        )}

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
            {/* Acts on what is on screen, not on every Page that exists.
                "Select all" quietly meaning forty live posts is not a button
                anyone should be able to press by accident. */}
            {visible.length > 0 && (
              <button
                className="linkbtn"
                type="button"
                onClick={() =>
                  setSelected((prev) => {
                    const allShown = visible.every((t) => prev.has(t.key));
                    const next = new Set(prev);
                    for (const t of visible) {
                      if (allShown) next.delete(t.key);
                      else next.add(t.key);
                    }
                    return next;
                  })
                }
              >
                {visible.every((t) => selected.has(t.key)) ? "Clear these" : "Select these"}
              </button>
            )}
          </div>

          {targets.length > TARGETS_SHOWN && (
            <input
              className="input input--sm pubform__search"
              value={query}
              placeholder={`Search ${targets.length} Pages by name, account or emulator`}
              onChange={(e) => setQuery(e.target.value)}
            />
          )}

          {targets.length === 0 ? (
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
              {visible.map(({ key, account, page, label }) => {
                const on = selected.has(key);
                return (
                  <button
                    key={key}
                    type="button"
                    className={`pick ${on ? "pick--on" : ""} ${
                      account.status === "connected" ? "" : "pick--dim"
                    }`.trim()}
                    onClick={() => toggle(key)}
                    aria-pressed={on}
                  >
                    <span className="pick__box" aria-hidden>
                      {on ? "✓" : ""}
                    </span>
                    <PlatformMark platform={account.platform} size={26} />
                    <span className="pick__text">
                      <span className="pick__name">{label}</span>
                      {/* Which signed-in app and which emulator this Page
                          posts through — with Pages from several accounts in
                          one list, the name alone does not say. */}
                      <span className="pick__meta">
                        {page ? `${account.name} · ` : ""}
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

          {/* Say what is not on screen. A picker that silently shows five of
              forty looks like an account list that lost most of itself. */}
          {hidden > 0 && (
            <div className="pubform__hint">
              <span>
                {matches
                  ? `${hidden} more not matching “${query.trim()}”`
                  : `Showing ${visible.length} of ${targets.length} — search to find the rest`}
              </span>
            </div>
          )}
          {matches?.length === 0 && (
            <div className="pubform__hint">
              <span>Nothing matches “{query.trim()}”.</span>
            </div>
          )}
        </div>

        <footer className="pubform__foot">
          <div className="pubform__summary">
            {paths.length === 0
              ? "Choose at least one video or photo"
              : selected.size === 0
                ? "Select at least one Page or account"
                : `${jobCount} ${jobCount === 1 ? "post" : "posts"} across ${
                    selected.size
                  } ${selected.size === 1 ? "target" : "targets"}`}
            {targets.some(
              (t) => selected.has(t.key) && t.account.status === "device_offline",
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

function FormatOption({
  format,
  current,
  onPick,
  title,
  detail,
}: {
  format: VideoFormat;
  current: VideoFormat;
  onPick: (f: VideoFormat) => void;
  title: string;
  detail: string;
}) {
  const on = current === format;
  return (
    <button
      className={`modeopt ${on ? "modeopt--on" : ""}`.trim()}
      type="button"
      onClick={() => onPick(format)}
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
