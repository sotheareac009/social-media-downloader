import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import {
  telegramChatAvatar,
  telegramListChats,
  telegramSendFile,
  type TelegramChat,
} from "@/lib/telegram";
import {
  uploadPickFiles,
  uploadVideoThumbnail,
  uploadTargets,
  uploadVideoMeta,
  uploadYoutube,
  uploadYoutubeChannels,
  type Privacy,
  type UploadTarget,
  type YoutubeChannel,
  uploadTiktok,
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

/** Read a local file's bytes through the asset protocol. */
async function readFileBytes(path: string): Promise<Uint8Array> {
  const res = await fetch(convertFileSrc(path));
  const buf = await res.arrayBuffer();
  return new Uint8Array(buf);
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
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [items, setItems] = useState<Item[]>([]);
  const [privacy, setPrivacy] = useState<Privacy>("unlisted");
  const [channels, setChannels] = useState<YoutubeChannel[] | null | "none">(null);
  const [chosenChannel, setChosenChannel] = useState<string | null>(null);
  // Which platform's destination dropdown is open (null = none).
  const [openMenu, setOpenMenu] = useState<string | null>(null);
  const [tgChats, setTgChats] = useState<TelegramChat[] | null | "error">(null);
  const [tgSelected, setTgSelected] = useState<Set<string>>(new Set());
  const [tgSearch, setTgSearch] = useState("");
  const [busy, setBusy] = useState(false);
  const [preview, setPreview] = useState<string | null>(null);

  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  // Close the destination dropdown on any outside click.
  useEffect(() => {
    if (!openMenu) return;
    const close = () => setOpenMenu(null);
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, [openMenu]);

  useEffect(() => {
    void (async () => {
      const list = await uploadTargets().catch(() => []);
      if (!mounted.current) return;
      setTargets(list);
      const firstReady = list.find((t) => t.ready);
      if (firstReady) setSelected(new Set([firstReady.id]));
    })();
  }, []);

  const chosen = useMemo(
    () => (targets ?? []).filter((t) => selected.has(t.id) && t.ready),
    [targets, selected],
  );
  const youtubeChosen = chosen.some((t) => t.id === "youtube");
  const telegramChosen = chosen.some((t) => t.id === "telegram");
  // File kind: video if any chosen platform takes video, else photo.
  const accepts =
    chosen.length === 0 || chosen.some((t) => t.accepts.includes("video"))
      ? "video"
      : "photo";

  const toggleTarget = useCallback((t: UploadTarget) => {
    if (!t.ready) return; // can't select a platform that isn't usable yet
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(t.id)) next.delete(t.id);
      else next.add(t.id);
      return next;
    });
  }, []);

  useEffect(() => {
    if (!youtubeChosen) {
      setChannels(null);
      return;
    }
    let alive = true;
    uploadYoutubeChannels()
      .then((cs) => {
        if (!alive) return;
        if (cs.length === 0) {
          setChannels("none");
        } else {
          setChannels(cs);
          setChosenChannel((prev) => prev ?? cs[0].id);
        }
      })
      .catch(() => alive && setChannels(null));
    return () => {
      alive = false;
    };
  }, [youtubeChosen]);

  const [tgRefreshing, setTgRefreshing] = useState(false);
  const loadTgChats = useCallback(async (showLoading: boolean) => {
    if (showLoading) setTgChats(null);
    setTgRefreshing(true);
    try {
      const cs = await telegramListChats();
      if (mounted.current) setTgChats(cs);
    } catch {
      // Keep any existing list on a refresh failure; only hard-fail the first load.
      if (mounted.current) setTgChats((prev) => (Array.isArray(prev) ? prev : "error"));
    } finally {
      if (mounted.current) setTgRefreshing(false);
    }
  }, []);

  // Load when Telegram is first chosen.
  useEffect(() => {
    if (telegramChosen) void loadTgChats(true);
  }, [telegramChosen, loadTgChats]);

  // Refresh whenever the picker opens, so a group created since shows up.
  useEffect(() => {
    if (openMenu === "telegram") void loadTgChats(false);
  }, [openMenu, loadTgChats]);

  // Per-platform destination list + the chosen one, keyed by platform id.
  const destinations = useMemo(
    (): Record<string, { id: string; name: string; avatar: string | null }[]> => ({
      youtube:
        Array.isArray(channels)
          ? channels.map((c) => ({ id: c.id, name: c.title, avatar: c.thumbnail }))
          : [],
    }),
    [channels],
  );
  const chosenId: Record<string, string | null> = { youtube: chosenChannel };
  const setChosen: Record<string, (id: string) => void> = { youtube: setChosenChannel };

  function activeDest(platform: string) {
    const list = destinations[platform] ?? [];
    return list.find((d) => d.id === chosenId[platform]) ?? list[0] ?? null;
  }

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
    if (chosen.length === 0 || items.length === 0) return;
    setBusy(true);
    setItems((prev) => prev.map((i) => ({ ...i, status: "pending", error: undefined })));

    let ok = 0;
    for (const item of items) {
      setItems((prev) =>
        prev.map((i) => (i.path === item.path ? { ...i, status: "uploading" } : i)),
      );
      const perTitle = item.title.trim() || baseName(item.path);
      const failures: string[] = [];

      // Send this file to every chosen platform.
      let bytes: Uint8Array | null = null; // read once, reused across chats
      for (const t of chosen) {
        try {
          if (t.id === "youtube") {
            await uploadYoutube(item.path, perTitle, item.description, privacy);
          } else if (t.id === "tiktok") {
            // Rust reads the file and handles TikTok's chunking rules; sending
            // the bytes through the webview would copy a large video twice for
            // no benefit.
            await uploadTiktok(item.path);
          } else if (t.id === "telegram") {
            if (tgSelected.size === 0) throw new Error("pick at least one chat");
            if (!bytes) bytes = await readFileBytes(item.path);
            const fileName = item.path.split("/").pop() ?? "video.mp4";
            // Dimensions/duration so Telegram keeps the correct aspect ratio.
            const meta = await uploadVideoMeta(item.path).catch(() => null);
            for (const chatId of tgSelected) {
              await telegramSendFile(chatId, bytes, fileName, item.description || perTitle, meta);
            }
          } else {
            throw new Error(`${t.name} upload isn't available yet.`);
          }
        } catch (e) {
          failures.push(`${t.name}: ${messageOf(e)}`);
        }
      }

      if (failures.length === 0) {
        ok += 1;
        setItems((prev) =>
          prev.map((i) => (i.path === item.path ? { ...i, status: "done" } : i)),
        );
      } else {
        setItems((prev) =>
          prev.map((i) =>
            i.path === item.path ? { ...i, status: "failed", error: failures.join(" · ") } : i,
          ),
        );
      }
    }

    if (mounted.current) setBusy(false);
    const failed = items.length - ok;
    const dests = chosen.map((t) => t.name).join(", ");
    if (failed === 0) toast("success", `Uploaded ${ok} to ${dests}.`);
    else toast(ok > 0 ? "info" : "error", `${ok} done, ${failed} with errors.`);
  }, [chosen, items, privacy, tgSelected, toast]);

  const canPublish = chosen.length > 0 && items.length > 0 && !busy;

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
            const isSel = selected.has(t.id) && t.ready;

            // Telegram is a container card: the header toggles the platform,
            // while a dropdown + chosen-chat chips live inside it.
            if (t.id === "telegram") {
              return (
                <div
                  key={t.id}
                  className={`up-target up-target--box ${isSel ? "up-target--active up-target--wide" : ""} ${t.ready ? "" : "up-target--off"}`.trim()}
                  style={{ ["--brand" as string]: brand }}
                >
                  <span className="up-target__edge" />
                  <button
                    type="button"
                    className="up-target__head"
                    onClick={() => toggleTarget(t)}
                    disabled={!t.ready}
                  >
                    <SourceLogo source="telegram" />
                    <span className="up-target__text">
                      <span className="up-target__name">Telegram</span>
                      <span className={`up-target__pill ${t.ready ? "up-target__pill--ok" : "up-target__pill--off"}`}>
                        {t.ready
                          ? tgSelected.size === 0
                            ? (<><CheckIcon size={10} /> Ready</>)
                            : `${tgSelected.size} chat${tgSelected.size === 1 ? "" : "s"}`
                          : "Not ready"}
                      </span>
                    </span>
                    {t.ready && (
                      <span className={`up-target__mark ${isSel ? "up-target__mark--on" : ""}`.trim()}>
                        {isSel && <CheckIcon size={12} />}
                      </span>
                    )}
                  </button>

                  {isSel && (
                    <div className="up-tg">
                      <button
                        type="button"
                        className={`up-tg__trigger ${openMenu === "telegram" ? "up-tg__trigger--open" : ""}`.trim()}
                        onClick={(e) => {
                          e.stopPropagation();
                          setOpenMenu((m) => (m === "telegram" ? null : "telegram"));
                        }}
                      >
                        <span>{tgSelected.size === 0 ? "Select groups & channels" : "Add or remove chats"}</span>
                        <span className="up-target__caret">▾</span>
                      </button>

                      {openMenu === "telegram" && (
                        <div className="up-tg__drop" onClick={(e) => e.stopPropagation()}>
                          <div className="up-tg__searchrow">
                            <input
                              className="up-tg__search"
                              placeholder="Search groups & channels…"
                              value={tgSearch}
                              autoFocus
                              onChange={(e) => setTgSearch(e.target.value)}
                            />
                            <button
                              type="button"
                              className="up-tg__refresh"
                              title="Refresh list"
                              disabled={tgRefreshing}
                              onClick={(e) => {
                                e.stopPropagation();
                                void loadTgChats(false);
                              }}
                            >
                              {tgRefreshing ? "…" : "↻"}
                            </button>
                          </div>
                          <div className="up-tg__list">
                            {tgChats === null && <div className="up-menu__note">Loading your chats…</div>}
                            {tgChats === "error" && <div className="up-menu__note">Couldn't load chats. Reconnect Telegram.</div>}
                            {Array.isArray(tgChats) && tgChats.length === 0 && <div className="up-menu__note">No groups or channels found.</div>}
                            {Array.isArray(tgChats) &&
                              tgChats
                                .filter((c) => c.title.toLowerCase().includes(tgSearch.trim().toLowerCase()))
                                .map((c) => {
                                  const on = tgSelected.has(c.id);
                                  return (
                                    <button
                                      key={c.id}
                                      type="button"
                                      className={`up-tg__item ${on ? "up-tg__item--on" : ""}`.trim()}
                                      onClick={() =>
                                        setTgSelected((prev) => {
                                          const n = new Set(prev);
                                          if (n.has(c.id)) n.delete(c.id);
                                          else n.add(c.id);
                                          return n;
                                        })
                                      }
                                    >
                                      <span className={`tg-picker__check ${on ? "tg-picker__check--on" : ""}`}>{on && <CheckIcon size={12} />}</span>
                                      <TgAvatar chatId={c.id} kind={c.kind} size={26} />
                                      <span className="tg-picker__name">{c.title}</span>
                                    </button>
                                  );
                                })}
                          </div>
                        </div>
                      )}

                      {tgSelected.size > 0 && Array.isArray(tgChats) && (
                        <div className="up-tg__chips">
                          {[...tgSelected].map((id) => {
                            const c = tgChats.find((x) => x.id === id);
                            if (!c) return null;
                            return (
                              <span key={id} className="up-tg__chip">
                                <TgAvatar chatId={c.id} kind={c.kind} size={18} />
                                <span className="up-tg__chipname">{c.title}</span>
                                <button
                                  type="button"
                                  className="up-tg__chipx"
                                  aria-label="Remove"
                                  onClick={() =>
                                    setTgSelected((prev) => {
                                      const n = new Set(prev);
                                      n.delete(id);
                                      return n;
                                    })
                                  }
                                >
                                  <XIcon size={11} />
                                </button>
                              </span>
                            );
                          })}
                        </div>
                      )}
                    </div>
                  )}
                </div>
              );
            }

            // Other platforms: single-select button cards.
            return (
              <button
                key={t.id}
                type="button"
                className={`up-target ${isSel ? "up-target--active" : ""} ${t.ready ? "" : "up-target--off"}`.trim()}
                style={{ ["--brand" as string]: brand }}
                onClick={() => toggleTarget(t)}
                title={t.reason ?? undefined}
                aria-pressed={isSel}
              >
                <span className="up-target__edge" />
                <SourceLogo source={t.id as SourceId} />
                <span className="up-target__text">
                  <span className="up-target__name">{t.name}</span>
                  {(() => {
                    const list = destinations[t.id] ?? [];
                    const dest = activeDest(t.id);
                    const many = list.length > 1;
                    if (!dest) {
                      return (
                        <span className={`up-target__pill ${t.ready ? "up-target__pill--ok" : "up-target__pill--off"}`}>
                          {t.ready ? (<><CheckIcon size={10} /> Ready</>) : "Not ready"}
                        </span>
                      );
                    }
                    return (
                      <span
                        className={`up-target__acct ${many ? "up-target__acct--menu" : ""}`.trim()}
                        role={many ? "button" : undefined}
                        onClick={(e) => {
                          if (!many) return;
                          e.stopPropagation();
                          setOpenMenu((m) => (m === t.id ? null : t.id));
                        }}
                      >
                        {dest.avatar && <img src={dest.avatar} alt="" referrerPolicy="no-referrer" />}
                        <span className="up-target__acctname">{dest.name}</span>
                        {many && <span className="up-target__caret">▾</span>}
                        {many && openMenu === t.id && (
                          <span className="up-menu" onClick={(e) => e.stopPropagation()}>
                            {list.map((d) => (
                              <button
                                key={d.id}
                                type="button"
                                className={`up-menu__item ${d.id === dest.id ? "up-menu__item--on" : ""}`.trim()}
                                onClick={(e) => {
                                  e.stopPropagation();
                                  setChosen[t.id]?.(d.id);
                                  setOpenMenu(null);
                                }}
                              >
                                {d.avatar && <img src={d.avatar} alt="" referrerPolicy="no-referrer" />}
                                <span>{d.name}</span>
                                {d.id === dest.id && <CheckIcon size={12} />}
                              </button>
                            ))}
                          </span>
                        )}
                      </span>
                    );
                  })()}
                </span>
                {t.ready && (
                  <span className={`up-target__mark ${isSel ? "up-target__mark--on" : ""}`.trim()}>
                    {isSel && <CheckIcon size={12} />}
                  </span>
                )}
              </button>
            );
          })}
        </div>

        {youtubeChosen && channels === "none" && (
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
            {items.map((item, i) => (
              <li
                key={item.path}
                className={`up-item up-item--${item.status}`}
                style={{ ["--i" as string]: i }}
              >
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

        {youtubeChosen && (
          <>
            <label className="tg-field__label" htmlFor="up-privacy" style={{ marginTop: 16 }}>
              YouTube visibility
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
              : chosen.length === 0
                ? "Pick a platform"
                : `Upload ${items.length > 1 ? `${items.length} ` : ""}to ${chosen.map((t) => t.name).join(", ")}`}
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

/** Lazily-loaded profile photo for a Telegram chat; emoji fallback. */
function TgAvatar({ chatId, kind, size = 22 }: { chatId: string; kind: "group" | "channel"; size?: number }) {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    telegramChatAvatar(chatId)
      .then((u) => alive && setUrl(u))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [chatId]);
  return url ? (
    <img className="tg-av" src={url} alt="" style={{ width: size, height: size }} />
  ) : (
    <span className="tg-av tg-av--ph" style={{ width: size, height: size, fontSize: size * 0.55 }}>
      {kind === "channel" ? "📢" : "👥"}
    </span>
  );
}

function messageOf(e: unknown): string {
  if (typeof e === "object" && e && "message" in e) {
    return String((e as { message: unknown }).message);
  }
  if (e instanceof Error) return e.message;
  return "Something went wrong.";
}
