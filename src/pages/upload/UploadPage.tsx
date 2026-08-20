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
  type Privacy,
  type UploadTarget,
  uploadTiktok,
  uploadX,
} from "@/lib/upload";
import {
  youtubeAccountAdd,
  youtubeAccountRemove,
  youtubeAccountUpload,
  youtubeAccountsList,
  type YoutubeAccount,
} from "@/lib/youtube";
import { Button } from "@/components/ui/Button";
import { useToast } from "@/components/ui/Toast";
import { CheckIcon, UploadIcon, XIcon } from "@/components/ui/icons";
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
  const [ytAccounts, setYtAccounts] = useState<YoutubeAccount[] | null>(null);
  const [ytSelected, setYtSelected] = useState<Set<string>>(new Set());
  const [ytAdding, setYtAdding] = useState(false);
  // Which platform's destination dropdown is open (null = none).
  const [openMenu, setOpenMenu] = useState<string | null>(null);
  const [tgChats, setTgChats] = useState<TelegramChat[] | null | "error">(null);
  const [tgSelected, setTgSelected] = useState<Set<string>>(new Set());
  const [tgSearch, setTgSearch] = useState("");
  const [busy, setBusy] = useState(false);
  const [preview, setPreview] = useState<string | null>(null);

  const mounted = useRef(true);
  const ytPreselected = useRef(false);
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

  const loadYtAccounts = useCallback(async () => {
    const list = await youtubeAccountsList().catch(() => []);
    if (!mounted.current) return;
    setYtAccounts(list);
    setYtSelected((prev) => {
      // Drop selections for accounts that no longer exist.
      const kept = new Set([...prev].filter((id) => list.some((a) => a.id === id)));
      // Convenience: on the very first load, pre-check the first account so a
      // single-account user can just upload. Never re-selects after that.
      if (!ytPreselected.current && list.length > 0) {
        ytPreselected.current = true;
        if (kept.size === 0) kept.add(list[0].id);
      }
      return kept;
    });
  }, []);
  useEffect(() => {
    void loadYtAccounts();
  }, [loadYtAccounts]);

  const chosen = useMemo(
    () => (targets ?? []).filter((t) => selected.has(t.id) && t.ready),
    [targets, selected],
  );
  const youtubeChosen = ytSelected.size > 0;
  const telegramChosen = chosen.some((t) => t.id === "telegram");
  // Keep the "youtube" platform in the selected set in lockstep with whether
  // any account is checked, so the rest of the flow (publish, summary) treats
  // it like the other platforms.
  useEffect(() => {
    setSelected((prev) => {
      const has = prev.has("youtube");
      if (youtubeChosen && !has) {
        const n = new Set(prev);
        n.add("youtube");
        return n;
      }
      if (!youtubeChosen && has) {
        const n = new Set(prev);
        n.delete("youtube");
        return n;
      }
      return prev;
    });
  }, [youtubeChosen]);

  // Add another Google account as an uploader (Google shows its chooser).
  const addYtAccount = useCallback(async () => {
    setYtAdding(true);
    try {
      const acct = await youtubeAccountAdd();
      await loadYtAccounts();
      if (mounted.current) {
        setYtSelected((prev) => new Set(prev).add(acct.id));
        // The youtube target flips to ready once an account exists.
        const list = await uploadTargets().catch(() => null);
        if (list && mounted.current) setTargets(list);
        toast("success", `Added ${acct.channel_title ?? acct.display_name}.`);
      }
    } catch (e) {
      const msg = messageOf(e);
      toast(/cancel/i.test(msg) ? "info" : "error", msg);
    } finally {
      if (mounted.current) setYtAdding(false);
    }
  }, [loadYtAccounts, toast]);

  const removeYtAccount = useCallback(
    async (id: string) => {
      try {
        await youtubeAccountRemove(id);
        setYtSelected((prev) => {
          const n = new Set(prev);
          n.delete(id);
          return n;
        });
        await loadYtAccounts();
        const list = await uploadTargets().catch(() => null);
        if (list && mounted.current) setTargets(list);
      } catch (e) {
        toast("error", messageOf(e));
      }
    },
    [loadYtAccounts, toast],
  );
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
            title: baseName(p).slice(0, 100),
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
            // YouTube is optional: if no account is ticked, just skip it rather
            // than failing the whole upload.
            if (ytSelected.size === 0) continue;
            const errs: string[] = [];
            for (const accountId of ytSelected) {
              try {
                await youtubeAccountUpload(accountId, item.path, perTitle, item.description, privacy);
              } catch (e) {
                const acct = (ytAccounts ?? []).find((a) => a.id === accountId);
                errs.push(`${acct?.channel_title ?? acct?.display_name ?? "account"}: ${messageOf(e)}`);
              }
            }
            if (errs.length > 0) throw new Error(errs.join(" · "));
          } else if (t.id === "tiktok") {
            // Rust reads the file and handles TikTok's chunking rules; sending
            // the bytes through the webview would copy a large video twice for
            // no benefit.
            await uploadTiktok(item.path);
          } else if (t.id === "x") {
            // Rust uploads the media and creates the post; caption is the
            // description, falling back to the title.
            await uploadX(item.path, item.description.trim() || perTitle);
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
  }, [chosen, items, privacy, tgSelected, ytSelected, ytAccounts, toast]);

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

            // YouTube is a container card holding one or more uploader accounts,
            // each independently selectable; "Add account" runs a Google login.
            if (t.id === "youtube") {
              const accounts = ytAccounts ?? [];
              return (
                <div
                  key={t.id}
                  className={`up-target up-target--box ${youtubeChosen ? "up-target--active up-target--wide" : ""}`.trim()}
                  style={{ ["--brand" as string]: brand }}
                >
                  <span className="up-target__edge" />
                  <div className="up-target__head" style={{ cursor: "default" }}>
                    <SourceLogo source="youtube" />
                    <span className="up-target__text">
                      <span className="up-target__name">YouTube</span>
                      <span className={`up-target__pill ${accounts.length > 0 ? "up-target__pill--ok" : "up-target__pill--off"}`}>
                        {ytSelected.size > 0
                          ? `${ytSelected.size} account${ytSelected.size === 1 ? "" : "s"}`
                          : accounts.length > 0
                            ? (<><CheckIcon size={10} /> Pick account(s)</>)
                            : "No account yet"}
                      </span>
                    </span>
                    {youtubeChosen && (
                      <span className="up-target__mark up-target__mark--on">
                        <CheckIcon size={12} />
                      </span>
                    )}
                  </div>

                  <div className="up-tg">
                    {accounts.length > 0 && (
                      <div className="up-tg__list" style={{ maxHeight: 220 }}>
                        {accounts.map((a) => {
                          const on = ytSelected.has(a.id);
                          return (
                            <div key={a.id} className={`up-tg__item ${on ? "up-tg__item--on" : ""}`.trim()}>
                              <button
                                type="button"
                                className="up-yt__pick"
                                onClick={() =>
                                  setYtSelected((prev) => {
                                    const n = new Set(prev);
                                    if (n.has(a.id)) n.delete(a.id);
                                    else n.add(a.id);
                                    return n;
                                  })
                                }
                              >
                                <span className={`tg-picker__check ${on ? "tg-picker__check--on" : ""}`}>{on && <CheckIcon size={12} />}</span>
                                {a.channel_avatar || a.avatar_url ? (
                                  <img className="tg-av" src={(a.channel_avatar || a.avatar_url)!} alt="" referrerPolicy="no-referrer" style={{ width: 26, height: 26 }} />
                                ) : (
                                  <span className="tg-av tg-av--ph" style={{ width: 26, height: 26, fontSize: 13 }}>▶</span>
                                )}
                                <span className="up-yt__meta">
                                  <span className="tg-picker__name">{a.channel_title ?? a.display_name}</span>
                                  {a.email && <span className="up-yt__sub">{a.email}</span>}
                                </span>
                              </button>
                              <button
                                type="button"
                                className="up-tg__chipx"
                                aria-label="Remove account"
                                title="Remove account"
                                onClick={() => void removeYtAccount(a.id)}
                              >
                                <XIcon size={12} />
                              </button>
                            </div>
                          );
                        })}
                      </div>
                    )}
                    <button
                      type="button"
                      className="btn btn--ghost btn--sm up-yt__add"
                      disabled={ytAdding}
                      onClick={() => void addYtAccount()}
                    >
                      {ytAdding ? "Opening Google…" : accounts.length > 0 ? "+ Add another account" : "+ Add a YouTube account"}
                    </button>
                  </div>
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
                  <span className={`up-target__pill ${t.ready ? "up-target__pill--ok" : "up-target__pill--off"}`}>
                    {t.ready ? (<><CheckIcon size={10} /> Ready</>) : "Not ready"}
                  </span>
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
