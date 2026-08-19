import { useCallback, useEffect, useRef, useState } from "react";
import {
  downloadCancel,
  downloadClearFinished,
  downloadEngineStatus,
  downloadGetDestination,
  downloadGetQuality,
  downloadInspectFormats,
  instagramConnect,
  instagramDisconnect,
  instagramStatus,
  downloadSetCompatible,
  downloadSetQuality,
  downloadList,
  downloadRemove,
  downloadReveal,
  downloadStartMany,
  downloadSubmit,
  downloadMessage,
  downloadBrowseDestination,
  downloadResetDestination,
  downloadSetDestination,
  isTerminal,
  subscribeToDownloadEvents,
  type Destination,
  type EngineStatus,
  type JobView,
  type FormatReport,
  type ProfileListing,
  type Quality,
  type QualitySettings,
  type SessionStatus,
} from "@/lib/download";
import { toAuthError } from "@/lib/auth";
import { DestinationBar } from "@/components/downloads/DestinationBar";
import { EngineNotice } from "@/components/downloads/EngineNotice";
import { InstagramSessionCard } from "@/components/downloads/InstagramSession";
import { JobCard } from "@/components/downloads/JobCard";
import { ProfileCard } from "@/components/downloads/ProfileCard";
import { QualityPicker } from "@/components/downloads/QualityPicker";
import { FormatPanel } from "@/components/downloads/FormatPanel";
import {
  countJobs,
  QueueSummary,
  type QueueFilter,
} from "@/components/downloads/QueueSummary";
import { UrlBar } from "@/components/downloads/UrlBar";
import { useToast } from "@/components/ui/Toast";
import { DownloadIcon, GlobeIcon, ShieldIcon } from "@/components/ui/icons";

/**
 * Above this many jobs, per-job toasts are suppressed and the summary panel
 * reports the batch instead.
 */
const BATCH_TOAST_LIMIT = 5;

export function DownloadsPage() {
  const toast = useToast();
  const [engine, setEngine] = useState<EngineStatus | null>(null);
  const [rechecking, setRechecking] = useState(false);
  const [jobs, setJobs] = useState<JobView[]>([]);
  const [dest, setDest] = useState<Destination | null>(null);
  // A folder the user has browsed to but not yet committed.
  const [pendingDest, setPendingDest] = useState<string | null>(null);
  const [destBusy, setDestBusy] = useState(false);
  const [quality, setQuality] = useState<QualitySettings | null>(null);
  const [qualityBusy, setQualityBusy] = useState(false);
  const [igSession, setIgSession] = useState<SessionStatus | null>(null);
  const [igBusy, setIgBusy] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  // Profiles found in a paste, waiting for the user to confirm the count.
  const [profiles, setProfiles] = useState<ProfileListing[]>([]);
  const [confirming, setConfirming] = useState<string | null>(null);
  // A single pasted link, inspected so its real quality tiers can be offered.
  const [report, setReport] = useState<FormatReport | null>(null);
  const [reportFor, setReportFor] = useState<string | null>(null);
  const [pickedQuality, setPickedQuality] = useState<Quality>("best");
  // Held in a ref because the debounce has to be cancelled from the *next*
  // keystroke, not by a React cleanup — the caller ignores our return value.
  const inspectTimer = useRef<number | null>(null);
  const [filter, setFilter] = useState<QueueFilter>("all");
  const [retrying, setRetrying] = useState(false);
  // Armed by "Retry when finished": fires once nothing is in progress.
  const [retryWhenIdle, setRetryWhenIdle] = useState(false);

  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  // Read inside event handlers, which close over their first render.
  const jobCount = useRef(0);
  useEffect(() => {
    jobCount.current = jobs.length;
  }, [jobs.length]);

  /** Replace one job in place, or prepend it if it's new. */
  const upsert = useCallback((job: JobView) => {
    setJobs((prev) => {
      const i = prev.findIndex((j) => j.id === job.id);
      if (i === -1) return [job, ...prev];
      const next = [...prev];
      next[i] = job;
      return next;
    });
  }, []);

  const refreshEngine = useCallback(async () => {
    const status = await downloadEngineStatus();
    if (mounted.current) setEngine(status);
    return status;
  }, []);

  useEffect(() => {
    void (async () => {
      try {
        const [status, list, destination, q] = await Promise.all([
          downloadEngineStatus(),
          downloadList(),
          downloadGetDestination(),
          downloadGetQuality(),
        ]);
        instagramStatus()
          .then((st) => mounted.current && setIgSession(st))
          .catch(() => {});
        if (!mounted.current) return;
        setEngine(status);
        setJobs(list);
        setDest(destination);
        setQuality(q);
      } catch {
        if (mounted.current) {
          setEngine({
            available: false,
            path: null,
            version: null,
            has_ffmpeg: false,
            ffmpeg_path: null,
          });
        }
      }
    })();
  }, []);

  // Progress is a separate, high-frequency event carrying only the numbers, so
  // it's merged field-by-field rather than replacing the whole job.
  useEffect(() => {
    const pending = subscribeToDownloadEvents({
      onCreated: upsert,
      onUpdated: upsert,
      // One toast per job is right for a single paste and unbearable for a
      // 133-video profile, so past a handful the summary panel is the report.
      onFinished: (job) => {
        upsert(job);
        if (jobCount.current <= BATCH_TOAST_LIMIT) {
          toast("success", `${job.title ?? "Video"} saved.`);
        }
      },
      onFailed: (job) => {
        upsert(job);
        if (jobCount.current <= BATCH_TOAST_LIMIT) {
          toast(
            "error",
            downloadMessage(job.error_code, job.error_message ?? "Download failed."),
          );
        }
      },
      onProgress: (p) =>
        setJobs((prev) =>
          prev.map((j) =>
            j.id === p.id
              ? {
                  ...j,
                  downloaded_bytes: p.downloaded_bytes,
                  total_bytes: p.total_bytes ?? j.total_bytes,
                  speed_bps: p.speed_bps,
                  eta_seconds: p.eta_seconds,
                  fraction: p.fraction,
                }
              : j,
          ),
        ),
    });
    return () => {
      void pending.then((unlisten) => unlisten());
    };
  }, [upsert, toast]);

  /**
   * Submit a whole paste — any mix of video links and profile links.
   *
   * Videos queue immediately. Profiles come back as listings the user has to
   * confirm, since one line can expand into a hundred downloads. One bad link
   * never discards the good ones.
   */
  const start = useCallback(
    async (urls: string[]) => {
      setSubmitting(true);
      try {
        const result = await downloadSubmit(urls);
        result.queued.forEach(upsert);
        if (result.profiles.length > 0) {
          // Replace any earlier listing for the same profile rather than
          // stacking duplicates when someone pastes it twice.
          setProfiles((prev) => [
            ...result.profiles,
            ...prev.filter(
              (p) => !result.profiles.some((n) => n.profile_url === p.profile_url),
            ),
          ]);
        }
        if (result.rejected.length > 0) {
          if (result.rejected.some((r) => r.code === "engine_missing")) {
            void refreshEngine();
          }
          // Identical reasons collapse: ten non-Facebook links are one complaint.
          const unique = [
            ...new Set(
              result.rejected.map((r) => downloadMessage(r.code, r.message)),
            ),
          ];
          const ok = result.queued.length + result.profiles.length;
          toast(
            "error",
            ok > 0
              ? `${ok} accepted, ${result.rejected.length} skipped — ${unique.join(" ")}`
              : unique.join(" "),
          );
        }
      } catch (e) {
        const err = toAuthError(e);
        toast("error", downloadMessage(err.code, err.message));
      } finally {
        if (mounted.current) setSubmitting(false);
      }
    },
    [upsert, toast, refreshEngine],
  );

  /** Queue every video from a confirmed profile. */
  const confirmProfile = useCallback(
    async (listing: ProfileListing) => {
      setConfirming(listing.profile_url);
      try {
        const created = await downloadStartMany(listing.entries.map((e) => e.url));
        created.forEach(upsert);
        setProfiles((prev) =>
          prev.filter((p) => p.profile_url !== listing.profile_url),
        );
        toast("success", `Queued ${created.length} videos from @${listing.uploader}.`);
      } catch (e) {
        toast("error", toAuthError(e).message);
      } finally {
        if (mounted.current) setConfirming(null);
      }
    },
    [upsert, toast],
  );

  const cancel = useCallback(
    async (id: string) => {
      try {
        upsert(await downloadCancel(id));
      } catch (e) {
        toast("error", toAuthError(e).message);
      }
    },
    [upsert, toast],
  );

  const remove = useCallback(
    async (id: string) => {
      try {
        await downloadRemove(id);
        setJobs((prev) => prev.filter((j) => j.id !== id));
      } catch (e) {
        toast("error", toAuthError(e).message);
      }
    },
    [toast],
  );

  const reveal = useCallback(
    async (path: string) => {
      try {
        await downloadReveal(path);
      } catch {
        toast("error", "Couldn't open the folder.");
      }
    },
    [toast],
  );

  /**
   * Browse only — nothing is applied until Save. A dismissed picker resolves
   * to null, which is the user changing their mind, not an error to apologise
   * for with a toast.
   */
  const browseFolder = useCallback(async () => {
    setDestBusy(true);
    try {
      const picked = await downloadBrowseDestination();
      if (!picked) return;
      // Picking the folder already in use is a no-op, not a pending change.
      if (picked === dest?.path) return;
      setPendingDest(picked);
    } catch (e) {
      const err = toAuthError(e);
      toast("error", downloadMessage(err.code, err.message));
    } finally {
      if (mounted.current) setDestBusy(false);
    }
  }, [toast, dest]);

  /** Commit the browsed folder. This is the step that writes it to disk. */
  const saveFolder = useCallback(async () => {
    if (!pendingDest) return;
    setDestBusy(true);
    try {
      const next = await downloadSetDestination(pendingDest);
      setDest(next);
      setPendingDest(null);
      toast("success", "Saved. This folder will still be used next time you open the app.");
    } catch (e) {
      const err = toAuthError(e);
      // Keep the proposal on screen so the path isn't lost to a fixable error.
      toast("error", downloadMessage(err.code, err.message));
    } finally {
      if (mounted.current) setDestBusy(false);
    }
  }, [pendingDest, toast]);

  const resetFolder = useCallback(async () => {
    setDestBusy(true);
    try {
      const next = await downloadResetDestination();
      setDest(next);
      setPendingDest(null);
      toast("info", "Back to the default download folder.");
    } catch (e) {
      toast("error", toAuthError(e).message);
    } finally {
      if (mounted.current) setDestBusy(false);
    }
  }, [toast]);

  const changeQuality = useCallback(
    async (next: Quality) => {
      setQualityBusy(true);
      try {
        await downloadSetQuality(next);
        setQuality((prev) => (prev ? { ...prev, selected: next } : prev));
      } catch (e) {
        toast("error", toAuthError(e).message);
      } finally {
        if (mounted.current) setQualityBusy(false);
      }
    },
    [toast],
  );

  /**
   * Inspect a single pasted link after a pause in typing.
   *
   * Only for one link at a time: probing ten pasted URLs would fire ten
   * network requests while someone is still editing. Failures are swallowed —
   * this is an enhancement, and the paste box must keep working without it.
   */
  const onDraftChange = useCallback(
    (urls: string[]) => {
      if (inspectTimer.current !== null) {
        window.clearTimeout(inspectTimer.current);
        inspectTimer.current = null;
      }

      if (urls.length !== 1) {
        setReport(null);
        setReportFor(null);
        return;
      }
      const url = urls[0];
      if (url === reportFor) return;
      setReportFor(url);
      setReport(null);
      if (!/^https?:\/\//i.test(url)) return;

      inspectTimer.current = window.setTimeout(() => {
        downloadInspectFormats(url)
          .then((r) => {
            // A slow probe can land after the box moved on; only apply it if
            // this is still the link being looked at.
            if (!mounted.current) return;
            setReport(r);
            setPickedQuality(quality?.selected ?? "best");
          })
          .catch(() => {});
      }, 700);
    },
    [reportFor, quality],
  );

  const downloadInspected = useCallback(async () => {
    if (!reportFor) return;
    setSubmitting(true);
    try {
      const result = await downloadSubmit([reportFor], pickedQuality);
      result.queued.forEach(upsert);
      setReport(null);
      setReportFor(null);
    } catch (e) {
      const err = toAuthError(e);
      toast("error", downloadMessage(err.code, err.message));
    } finally {
      if (mounted.current) setSubmitting(false);
    }
  }, [reportFor, pickedQuality, upsert, toast]);

  /**
   * Opens the Instagram login window and waits for it. A cancelled sign-in is
   * a normal outcome, not something to report as a failure.
   */
  const connectInstagram = useCallback(async () => {
    setIgBusy(true);
    try {
      setIgSession(await instagramConnect());
      toast("success", "Instagram connected. Reels can now be downloaded.");
    } catch (e) {
      const err = toAuthError(e);
      if (err.code !== "cancelled") {
        toast("error", downloadMessage(err.code, err.message));
      }
    } finally {
      if (mounted.current) setIgBusy(false);
    }
  }, [toast]);

  const disconnectInstagram = useCallback(async () => {
    setIgBusy(true);
    try {
      setIgSession(await instagramDisconnect());
      toast("info", "Instagram session removed from your keychain.");
    } catch (e) {
      toast("error", toAuthError(e).message);
    } finally {
      if (mounted.current) setIgBusy(false);
    }
  }, [toast]);

  const toggleCompatible = useCallback(
    async (on: boolean) => {
      setQualityBusy(true);
      try {
        await downloadSetCompatible(on);
        setQuality((prev) => (prev ? { ...prev, prefer_compatible: on } : prev));
      } catch (e) {
        toast("error", toAuthError(e).message);
      } finally {
        if (mounted.current) setQualityBusy(false);
      }
    },
    [toast],
  );

  const clearFinished = useCallback(async () => {
    await downloadClearFinished();
    setJobs((prev) => prev.filter((j) => !isTerminal(j.status)));
    setFilter("all");
  }, []);

  /**
   * Re-queue everything that failed.
   *
   * The old rows are dropped first so the retry doesn't sit next to the
   * failure it replaces — one row per video, showing its latest attempt.
   */
  const retryFailed = useCallback(async () => {
    const failed = jobs.filter((j) => j.status === "failed");
    if (failed.length === 0) return;
    setRetryWhenIdle(false);
    setRetrying(true);
    try {
      const created = await downloadStartMany(failed.map((j) => j.url));
      await Promise.all(failed.map((j) => downloadRemove(j.id).catch(() => {})));
      setJobs((prev) => {
        const gone = new Set(failed.map((j) => j.id));
        return [...created, ...prev.filter((j) => !gone.has(j.id))];
      });
      setFilter("all");
      toast("info", `Retrying ${created.length} failed downloads.`);
    } catch (e) {
      toast("error", toAuthError(e).message);
    } finally {
      if (mounted.current) setRetrying(false);
    }
  }, [jobs, toast]);

  const recheck = useCallback(async () => {
    setRechecking(true);
    try {
      const status = await refreshEngine();
      downloadGetQuality()
        .then((q) => mounted.current && setQuality(q))
        .catch(() => {});
      toast(
        status.available ? "success" : "info",
        status.available
          ? `Found yt-dlp ${status.version ?? ""}`.trim()
          : "Still not finding yt-dlp.",
      );
    } finally {
      if (mounted.current) setRechecking(false);
    }
  }, [refreshEngine, toast]);

  /**
   * Retrying while other downloads are still running would put the retries
   * straight back into the contention that failed them, so the button arms
   * instead and this fires when the queue drains.
   */
  const requestRetry = useCallback(() => {
    const active = jobs.some((j) => !isTerminal(j.status));
    if (active) setRetryWhenIdle(true);
    else void retryFailed();
  }, [jobs, retryFailed]);

  useEffect(() => {
    // `retrying` guards the window between firing and the new jobs appearing,
    // when the queue still looks idle and would fire a second time.
    if (!retryWhenIdle || retrying) return;
    const active = jobs.some((j) => !isTerminal(j.status));
    const failed = jobs.some((j) => j.status === "failed");
    if (active) return;
    // Nothing left to retry - disarm quietly rather than firing on an empty set.
    if (!failed) {
      setRetryWhenIdle(false);
      return;
    }
    void retryFailed();
  }, [retryWhenIdle, retrying, jobs, retryFailed]);

  const finishedCount = jobs.filter((j) => isTerminal(j.status)).length;
  const engineReady = engine?.available === true;
  const counts = countJobs(jobs);
  const visibleJobs = jobs.filter((j) => {
    if (filter === "all") return true;
    if (filter === "active") return !isTerminal(j.status);
    if (filter === "completed") return j.status === "completed";
    return j.status === "failed";
  });

  return (
    <div className="page">
      <header className="page__header rise">
        <span className="page__eyebrow">
          <DownloadIcon size={12} />
          Downloads
        </span>
        <h1 className="page__title">Download public videos</h1>
        <p className="page__lede">
          Paste a link — or a whole list, one per line — for public YouTube,
          Facebook and TikTok videos, Shorts and reels. A TikTok profile or
          YouTube channel link grabs everything they've posted. No account
          needed: these are posts anyone can already open in a browser.
        </p>
      </header>

      {engine && (
        <div className="rise" style={{ marginBottom: 16 }}>
          <EngineNotice
            status={engine}
            onRecheck={() => void recheck()}
            rechecking={rechecking}
          />
        </div>
      )}

      {igSession && (
        <div className="rise" style={{ marginBottom: 14, animationDelay: "40ms" }}>
          <InstagramSessionCard
            status={igSession}
            busy={igBusy}
            onConnect={() => void connectInstagram()}
            onDisconnect={() => void disconnectInstagram()}
          />
        </div>
      )}

      <div className="rise" style={{ animationDelay: "60ms" }}>
        <UrlBar
          onDraftChange={onDraftChange}
          onSubmit={(urls) => void start(urls)}
          busy={submitting}
          disabled={!engineReady}
        />
      </div>

      {quality && (
        <QualityPicker
          settings={quality}
          busy={qualityBusy}
          onChange={(q) => void changeQuality(q)}
          onToggleCompatible={(on) => void toggleCompatible(on)}
        />
      )}

      {dest && (
        <DestinationBar
          destination={dest}
          pending={pendingDest}
          busy={destBusy}
          onBrowse={() => void browseFolder()}
          onSave={() => void saveFolder()}
          onDiscard={() => setPendingDest(null)}
          onReset={() => void resetFolder()}
          onOpen={() => void reveal(dest.path)}
        />
      )}

      {report && (
        <div className="rise" style={{ marginTop: 14 }}>
          <FormatPanel
            report={report}
            chosen={pickedQuality}
            busy={submitting}
            hasFfmpeg={engine?.has_ffmpeg ?? false}
            onChoose={setPickedQuality}
            onDownload={() => void downloadInspected()}
            onDismiss={() => {
              setReport(null);
              setReportFor(null);
            }}
          />
        </div>
      )}

      {profiles.length > 0 && (
        <div className="stack" style={{ marginTop: 16 }}>
          {profiles.map((listing) => (
            <ProfileCard
              key={listing.profile_url}
              listing={listing}
              busy={confirming === listing.profile_url}
              onConfirm={() => void confirmProfile(listing)}
              onDismiss={() =>
                setProfiles((prev) =>
                  prev.filter((p) => p.profile_url !== listing.profile_url),
                )
              }
            />
          ))}
        </div>
      )}

      <div className="queue">
        <div className="queue__head">
          <h2 className="queue__title">
            Queue {jobs.length > 0 && <span>({jobs.length})</span>}
          </h2>
          {finishedCount > 0 && (
            <button
              className="btn btn--ghost btn--sm"
              type="button"
              onClick={() => void clearFinished()}
            >
              Clear finished
            </button>
          )}
        </div>

        {jobs.length > 1 && (
          <QueueSummary
            counts={counts}
            filter={filter}
            onFilter={setFilter}
            onRetryFailed={requestRetry}
            retrying={retrying}
            armed={retryWhenIdle}
            onDisarm={() => setRetryWhenIdle(false)}
          />
        )}

        {jobs.length === 0 ? (
          <EmptyQueue engineReady={engineReady} />
        ) : visibleJobs.length === 0 ? (
          <p className="queue__none">Nothing matches that filter.</p>
        ) : (
          <div className="stack">
            {visibleJobs.map((job, i) => (
              <div
                key={job.id}
                className="rise"
                style={{ animationDelay: `${Math.min(i, 6) * 40}ms` }}
              >
                <JobCard
                  job={job}
                  onCancel={() => void cancel(job.id)}
                  onRemove={() => void remove(job.id)}
                  onReveal={() => job.output_path && void reveal(job.output_path)}
                />
              </div>
            ))}
          </div>
        )}
      </div>

      <PublicOnlyNote />
    </div>
  );
}

function EmptyQueue({ engineReady }: { engineReady: boolean }) {
  return (
    <div className="empty">
      <span className="empty__icon">
        <GlobeIcon size={20} />
      </span>
      <div className="empty__title">
        {engineReady ? "Nothing downloading yet" : "Install yt-dlp to begin"}
      </div>
      <p className="empty__text">
        {engineReady
          ? "Paste a link above — or several at once, one per line. YouTube videos and Shorts, Facebook watch pages, reels and share links, and TikTok videos. A channel or profile link grabs everything they've posted."
          : "Once the engine is installed, paste a link above and it'll appear here."}
      </p>
    </div>
  );
}

/**
 * States the boundary in the product itself, not just in the code comments.
 * People will reasonably assume that connecting an account unlocks private
 * posts — it doesn't, and it's better to say so before they try.
 */
function PublicOnlyNote() {
  return (
    <section className="assurance rise" style={{ animationDelay: "160ms" }}>
      <div className="assurance__title">
        <ShieldIcon size={14} />
        What this can and can't reach
      </div>
      <ul className="assurance__list">
        {[
          "Only public posts — the same ones you could open in a private browser window without logging in.",
          "YouTube, Facebook and TikTok download with no session at all — no cookies, no token.",
          "Instagram refuses anonymous requests, so it uses the session you sign into here. Your browser profile is never read.",
          "Connecting an account on the Accounts page does not unlock private videos, and isn't required here.",
          "You're responsible for how you use what you download — copyright and each platform's terms still apply.",
        ].map((line) => (
          <li key={line}>
            <span className="assurance__tick">•</span>
            <span>{line}</span>
          </li>
        ))}
      </ul>
    </section>
  );
}
