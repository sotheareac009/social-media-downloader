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
import { AlertIcon, CheckIcon, UploadIcon, XIcon } from "@/components/ui/icons";
import { SourceLogo, SOURCE_COLOR, type SourceId } from "@/components/home/SourceLogo";
import { QueuePager } from "@/components/downloads/QueuePager";
import { isUploadTargetHidden } from "@/lib/flags";

/**
 * Items per page.
 *
 * Small, because each row is a card with a thumbnail, a title and a
 * description box - twenty of them is a page nobody can read, and every one
 * holds a live thumbnail.
 */
const UPLOAD_PAGE_SIZE = 8;

type ItemStatus = "pending" | "uploading" | "done" | "failed";
interface Item {
  path: string;
  title: string;
  description: string;
  status: ItemStatus;
  error?: string;
}

interface PlatformResult {
  name: string;
  ok: number;
  fail: number;
  error?: string;
}
interface UploadResult {
  totalVideos: number;
  videosOk: number;
  platforms: PlatformResult[];
}


/** One destination for one file: the unit that succeeds, fails, and retries. */
type UploadJobStatus = "queued" | "uploading" | "done" | "failed";

/**
 * A single upload.
 *
 * One job per (file × destination) rather than per file. That granularity is
 * what makes retry honest: when a video reaches TikTok but not one of three
 * YouTube channels, retrying the *file* would re-post it to TikTok and to the
 * two channels that already have it. Retrying the job that failed posts it
 * exactly once, where it is missing.
 */
interface UploadJob {
  /** Stable across retries, so a re-run updates its row instead of adding one. */
  id: string;
  path: string;
  fileName: string;
  targetId: string;
  targetName: string;
  /** Set for destinations that fan out: one YouTube channel, one Telegram chat. */
  subId?: string;
  subLabel?: string;
  title: string;
  description: string;
  status: UploadJobStatus;
  error?: string;
  finishedAt?: number;
}

type JobTab = "all" | "active" | "done" | "failed";

/** Read a local file's bytes through the asset protocol. */
async function readFileBytes(path: string): Promise<Uint8Array> {
  const res = await fetch(convertFileSrc(path));
  const buf = await res.arrayBuffer();
  return new Uint8Array(buf);
}

/**
 * Drops the destinations this build is configured not to offer.
 *
 * Applied at every load, so a hidden platform never reaches the selector, the
 * chosen set, or the publish loop — there is one source of truth for what
 * exists rather than a filter per render.
 */
function visibleTargets(list: UploadTarget[]): UploadTarget[] {
  return list.filter((t) => !isUploadTargetHidden(t.id));
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
  const [result, setResult] = useState<UploadResult | null>(null);
  // Every destination attempted this session, newest run last. Kept across
  // runs so a retry updates the row that failed rather than starting a fresh
  // list and hiding what already worked.
  const [jobs, setJobs] = useState<UploadJob[]>([]);
  const [jobTab, setJobTab] = useState<JobTab>("all");
  const [preview, setPreview] = useState<string | null>(null);
  const [page, setPage] = useState(0);

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
      const list = visibleTargets(await uploadTargets().catch(() => []));
      if (!mounted.current) return;
      setTargets(list);
      const firstReady = list.find((t) => t.ready);
      if (firstReady) setSelected(new Set([firstReady.id]));
    })();
  }, []);

  const loadYtAccounts = useCallback(async () => {
    // A build with YouTube hidden has no card to fill, and no reason to touch
    // the stored uploader accounts.
    if (isUploadTargetHidden("youtube")) return;
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
        if (list && mounted.current) setTargets(visibleTargets(list));
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
        if (list && mounted.current) setTargets(visibleTargets(list));
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

  /**
   * Upload `only` those items, or everything when it is omitted.
   *
   * The subset is what makes retry meaningful: re-running the whole list after
   * two failures would re-upload every video that already succeeded, which on
   * YouTube means duplicates nobody asked for.
   */
  /** Expand the chosen files and destinations into one job per pairing. */
  const buildJobs = useCallback(
    (queue: Item[]): UploadJob[] => {
      const out: UploadJob[] = [];
      for (const item of queue) {
        const title = item.title.trim() || baseName(item.path);
        const base = {
          path: item.path,
          fileName: baseName(item.path),
          title,
          description: item.description,
          status: "queued" as UploadJobStatus,
        };
        for (const t of chosen) {
          if (t.id === "youtube") {
            // No channel ticked is a skip, not a failure — the same rule the
            // single-shot version used.
            for (const accountId of ytSelected) {
              const acct = (ytAccounts ?? []).find((a) => a.id === accountId);
              out.push({
                ...base,
                id: `${item.path}::youtube::${accountId}`,
                targetId: t.id,
                targetName: t.name,
                subId: accountId,
                subLabel: acct?.channel_title ?? acct?.display_name ?? "channel",
              });
            }
          } else if (t.id === "telegram") {
            for (const chatId of tgSelected) {
              const chat = Array.isArray(tgChats)
                ? tgChats.find((c) => String(c.id) === chatId)
                : undefined;
              out.push({
                ...base,
                id: `${item.path}::telegram::${chatId}`,
                targetId: t.id,
                targetName: t.name,
                subId: chatId,
                subLabel: chat?.title ?? "chat",
              });
            }
          } else {
            out.push({
              ...base,
              id: `${item.path}::${t.id}`,
              targetId: t.id,
              targetName: t.name,
            });
          }
        }
      }
      return out;
    },
    [chosen, tgChats, tgSelected, ytAccounts, ytSelected],
  );

  /**
   * Send one file to one destination. Throws on failure; the caller records it.
   *
   * `bytesCache` exists because Telegram uploads go through the webview, so the
   * file has to be read into JS. Three chats for one video is one read, not
   * three — a 300 MB clip read per chat is a visible stall.
   */
  const executeJob = useCallback(
    async (job: UploadJob, bytesCache: Map<string, Uint8Array>) => {
      if (job.targetId === "youtube") {
        await youtubeAccountUpload(
          job.subId!,
          job.path,
          job.title,
          job.description,
          privacy,
        );
      } else if (job.targetId === "tiktok") {
        // Rust reads the file and handles TikTok's chunking rules; sending the
        // bytes through the webview would copy a large video twice for nothing.
        await uploadTiktok(job.path);
      } else if (job.targetId === "x") {
        await uploadX(job.path, job.description.trim() || job.title);
      } else if (job.targetId === "telegram") {
        let bytes = bytesCache.get(job.path);
        if (!bytes) {
          bytes = await readFileBytes(job.path);
          bytesCache.set(job.path, bytes);
        }
        const fileName = job.path.split(/[\\/]/).pop() ?? "video.mp4";
        // Dimensions/duration so Telegram keeps the correct aspect ratio.
        const meta = await uploadVideoMeta(job.path).catch(() => null);
        await telegramSendFile(
          job.subId!,
          bytes,
          fileName,
          job.description || job.title,
          meta,
        );
      } else {
        throw new Error(`${job.targetName} upload isn't available yet.`);
      }
    },
    [privacy],
  );

  /**
   * Run a set of jobs one at a time, publishing each result as it lands.
   *
   * Sequential on purpose: these are large uploads over one connection, and
   * running them at once makes every one slower while making the progress
   * meaningless.
   */
  const runJobs = useCallback(
    async (list: UploadJob[]) => {
      if (list.length === 0) return;
      setBusy(true);
      setResult(null);

      const queued = list.map((j) => ({
        ...j,
        status: "queued" as UploadJobStatus,
        error: undefined,
        finishedAt: undefined,
      }));
      // Re-running a job replaces its row rather than appending a second one.
      setJobs((prev) => {
        const byId = new Map(prev.map((j) => [j.id, j]));
        for (const j of queued) byId.set(j.id, j);
        return [...byId.values()];
      });

      const touched = new Set(list.map((j) => j.path));
      setItems((prev) =>
        prev.map((i) =>
          touched.has(i.path) ? { ...i, status: "uploading", error: undefined } : i,
        ),
      );

      const patch = (id: string, next: Partial<UploadJob>) =>
        setJobs((prev) => prev.map((j) => (j.id === id ? { ...j, ...next } : j)));

      const bytesCache = new Map<string, Uint8Array>();
      const outcomes: UploadJob[] = [];

      for (const job of queued) {
        if (!mounted.current) return;
        patch(job.id, { status: "uploading" });
        try {
          await executeJob(job, bytesCache);
          const done = { ...job, status: "done" as UploadJobStatus, finishedAt: Date.now() };
          outcomes.push(done);
          patch(job.id, done);
        } catch (e) {
          const failed = {
            ...job,
            status: "failed" as UploadJobStatus,
            error: messageOf(e),
            finishedAt: Date.now(),
          };
          outcomes.push(failed);
          patch(job.id, failed);
        }
      }

      if (!mounted.current) return;

      // A file is done only when every destination it was sent to accepted it.
      setItems((prev) =>
        prev.map((i) => {
          const mine = outcomes.filter((j) => j.path === i.path);
          if (mine.length === 0) return i;
          const bad = mine.filter((j) => j.status === "failed");
          return bad.length === 0
            ? { ...i, status: "done", error: undefined }
            : {
                ...i,
                status: "failed",
                error: bad.map((j) => `${j.targetName}: ${j.error}`).join(" · "),
              };
        }),
      );

      const okJobs = outcomes.filter((j) => j.status === "done").length;
      const failedJobs = outcomes.length - okJobs;

      const tally: Record<string, PlatformResult> = {};
      for (const j of outcomes) {
        const r = (tally[j.targetId] ??= { name: j.targetName, ok: 0, fail: 0 });
        if (j.status === "done") r.ok += 1;
        else {
          r.fail += 1;
          if (!r.error && j.error) r.error = j.error;
        }
      }
      const files = new Set(outcomes.map((j) => j.path));
      const filesOk = [...files].filter((p) =>
        outcomes.filter((j) => j.path === p).every((j) => j.status === "done"),
      ).length;

      setBusy(false);
      setResult({
        totalVideos: files.size,
        videosOk: filesOk,
        platforms: Object.values(tally),
      });

      if (failedJobs === 0) {
        toast("success", `Uploaded ${okJobs} destination${okJobs === 1 ? "" : "s"}.`);
      } else {
        // Failures land in their own tab, so the toast points at it rather
        // than trying to spell out what went wrong.
        setJobTab("failed");
        toast(
          okJobs > 0 ? "info" : "error",
          `${okJobs} done, ${failedJobs} failed — see the Failed tab.`,
        );
      }
    },
    [executeJob, toast],
  );

  /**
   * Upload `only` those items, or everything when it is omitted.
   */
  const publish = useCallback(
    async (only?: Item[]) => {
      const queue = only ?? items;
      if (chosen.length === 0 || queue.length === 0) return;
      await runJobs(buildJobs(queue));
    },
    [buildJobs, chosen, items, runJobs],
  );

  /**
   * How the run is going, derived from the jobs themselves.
   *
   * Derived rather than counted alongside the loop: one source of truth means
   * the tab badges can never disagree with the rows under them, which is the
   * usual way a progress summary goes wrong.
   */
  const jobCounts = useMemo(() => {
    let active = 0;
    let done = 0;
    let failed = 0;
    for (const j of jobs) {
      if (j.status === "done") done += 1;
      else if (j.status === "failed") failed += 1;
      else active += 1;
    }
    return { all: jobs.length, active, done, failed };
  }, [jobs]);

  const visibleJobs = useMemo(() => {
    if (jobTab === "all") return jobs;
    if (jobTab === "done") return jobs.filter((j) => j.status === "done");
    if (jobTab === "failed") return jobs.filter((j) => j.status === "failed");
    return jobs.filter((j) => j.status === "queued" || j.status === "uploading");
  }, [jobTab, jobs]);

  /** Re-run specific jobs — the failed ones, or a single row. */
  const retryJobs = useCallback(
    async (ids: string[]) => {
      const again = jobs.filter((j) => ids.includes(j.id));
      if (again.length > 0) await runJobs(again);
    },
    [jobs, runJobs],
  );

  /**
   * What has happened so far, recomputed from the items themselves.
   *
   * Derived rather than counted alongside the upload loop: one source of truth
   * means the strip can never disagree with the rows underneath it, which is
   * the usual way a progress summary goes wrong.
   */
  const tally = useMemo(() => {
    let done = 0;
    let failed = 0;
    let uploading = 0;
    for (const i of items) {
      if (i.status === "done") done++;
      else if (i.status === "failed") failed++;
      else if (i.status === "uploading") uploading++;
    }
    return { done, failed, uploading, settled: done + failed };
  }, [items]);

  const failedItems = useMemo(
    () => items.filter((i) => i.status === "failed"),
    [items],
  );

  const pageCount = Math.max(1, Math.ceil(items.length / UPLOAD_PAGE_SIZE));
  const currentPage = Math.min(page, pageCount - 1);
  const pageStart = currentPage * UPLOAD_PAGE_SIZE;
  const pageItems = items.slice(pageStart, pageStart + UPLOAD_PAGE_SIZE);
  useEffect(() => {
    if (page !== currentPage) setPage(currentPage);
  }, [page, currentPage]);

  // Follow the upload: watching a run from the wrong page shows nothing
  // happening at all.
  useEffect(() => {
    if (!busy) return;
    const at = items.findIndex((i) => i.status === "uploading");
    if (at < 0) return;
    const wanted = Math.floor(at / UPLOAD_PAGE_SIZE);
    setPage((prev) => (prev === wanted ? prev : wanted));
  }, [busy, items]);

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
        {jobs.length > 0 && (
          <section className="upjobs">
            <div className="upjobs__head">
              <div className="upjobs__tabs" role="tablist">
                {(
                  [
                    ["all", "All", jobCounts.all],
                    ["active", "In progress", jobCounts.active],
                    ["done", "Uploaded", jobCounts.done],
                    ["failed", "Failed", jobCounts.failed],
                  ] as [JobTab, string, number][]
                ).map(([id, label, count]) => (
                  <button
                    key={id}
                    role="tab"
                    aria-selected={jobTab === id}
                    className={`upjobs__tab ${jobTab === id ? "upjobs__tab--on" : ""} ${
                      id === "failed" && count > 0 ? "upjobs__tab--bad" : ""
                    }`.trim()}
                    type="button"
                    onClick={() => setJobTab(id)}
                  >
                    {label}
                    <span className="upjobs__count">{count}</span>
                  </button>
                ))}
              </div>
              {/* Only offered when there is something to retry, and never
                  mid-run: re-sending a job that is still uploading is how a
                  video gets posted twice. */}
              {jobCounts.failed > 0 && (
                <Button
                  variant="ghost"
                  disabled={busy}
                  onClick={() =>
                    void retryJobs(
                      jobs.filter((j) => j.status === "failed").map((j) => j.id),
                    )
                  }
                >
                  Retry all failed ({jobCounts.failed})
                </Button>
              )}
            </div>

            <div className="upjobs__list">
              {visibleJobs.length === 0 ? (
                <div className="upjobs__empty">Nothing here yet.</div>
              ) : (
                visibleJobs.map((job) => (
                  <div key={job.id} className={`upjob upjob--${job.status}`}>
                    <span className="upjob__dot" aria-hidden />
                    <div className="upjob__text">
                      <div className="upjob__where">
                        {job.targetName}
                        {job.subLabel && (
                          <span className="upjob__sub"> · {job.subLabel}</span>
                        )}
                      </div>
                      <div className="upjob__file" title={job.path}>
                        {job.fileName}
                      </div>
                      {job.error && <div className="upjob__error">{job.error}</div>}
                    </div>
                    <span className="upjob__status">
                      {job.status === "uploading"
                        ? "Uploading…"
                        : job.status === "queued"
                          ? "Queued"
                          : job.status === "done"
                            ? "Uploaded"
                            : "Failed"}
                    </span>
                    {job.status === "failed" && (
                      <Button
                        variant="ghost"
                        disabled={busy}
                        onClick={() => void retryJobs([job.id])}
                      >
                        Retry
                      </Button>
                    )}
                  </div>
                ))
              )}
            </div>
          </section>
        )}

        {result && (
          <section className="summary up-summary">
            <div className="summary__row">
              <div className="stat stat--ok">
                <span className="stat__icon"><CheckIcon size={13} /></span>
                <span className="stat__value">{result.videosOk}</span>
                <span className="stat__label">Uploaded</span>
              </div>
              <div className="stat stat--bad">
                <span className="stat__icon"><AlertIcon size={13} /></span>
                <span className="stat__value">{result.totalVideos - result.videosOk}</span>
                <span className="stat__label">Failed</span>
              </div>
            </div>

            <div className="summary__foot">
              <span>
                Finished — {result.videosOk} of {result.totalVideos} video
                {result.totalVideos === 1 ? "" : "s"} uploaded
              </span>
            </div>

            <ul className="up-result__list">
              {result.platforms.map((pr) => {
                const total = pr.ok + pr.fail;
                const state = pr.fail === 0 ? "ok" : pr.ok === 0 ? "bad" : "partial";
                return (
                  <li key={pr.name} className={`up-result__row up-result__row--${state}`}>
                    <span className="up-result__plat">
                      {pr.fail === 0 ? <CheckIcon size={13} /> : <XIcon size={13} />}
                      {pr.name}
                    </span>
                    <span className="up-result__count">{pr.ok}/{total}</span>
                    {pr.fail > 0 && pr.error && (
                      <span className="up-result__err">{pr.error}</span>
                    )}
                  </li>
                );
              })}
            </ul>
          </section>
        )}
        <div className="up-files-head">
          <label className="tg-field__label" style={{ margin: 0 }}>
            {accepts === "video" ? "Videos" : "Photos"}
            {items.length > 0 && <span className="up-count"> ({items.length})</span>}
          </label>
          <div className="up-files-actions">
            {failedItems.length > 0 && !busy && (
              <button
                className="btn btn--ghost btn--sm"
                type="button"
                onClick={() => void publish(failedItems)}
                title="Upload only the ones that failed"
              >
                Retry {failedItems.length} failed
              </button>
            )}
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

        {/* Live, not a summary printed at the end: during a long run the only
            question is how it is going, and the rows alone do not answer it
            once the list is longer than a screen. */}
        {(busy || tally.settled > 0) && (
          <div className="up-tally">
            <div className="up-tally__counts">
              <span className="up-tally__ok">
                <CheckIcon size={12} /> {tally.done} uploaded
              </span>
              {tally.failed > 0 && (
                <span className="up-tally__bad">
                  <AlertIcon size={12} /> {tally.failed} failed
                </span>
              )}
              <span className="up-tally__rest">
                {busy
                  ? `${tally.settled} of ${items.length} done`
                  : `${items.length} total`}
              </span>
            </div>
            <span className="up-tally__track">
              <span
                className="up-tally__fill"
                style={{
                  width: `${items.length === 0 ? 0 : (tally.settled / items.length) * 100}%`,
                }}
              />
            </span>
          </div>
        )}

        {items.length === 0 ? (
          <p className="up-empty">No files added yet. Click “Add files” to choose one or more.</p>
        ) : (
          <ul className="up-list">
            {pageItems.map((item, i) => (
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

        {pageCount > 1 && (
          <QueuePager
            page={currentPage}
            pageCount={pageCount}
            from={pageStart + 1}
            to={pageStart + pageItems.length}
            total={items.length}
            onPage={setPage}
          />
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
