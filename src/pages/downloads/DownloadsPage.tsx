import { useCallback, useEffect, useRef, useState } from "react";
import {
  downloadCancel,
  downloadClearFinished,
  downloadEngineStatus,
  downloadGetDestination,
  downloadList,
  downloadRemove,
  downloadReveal,
  downloadStart,
  downloadMessage,
  downloadBrowseDestination,
  downloadResetDestination,
  downloadSetDestination,
  isTerminal,
  subscribeToDownloadEvents,
  type Destination,
  type EngineStatus,
  type JobView,
} from "@/lib/download";
import { toAuthError } from "@/lib/auth";
import { DestinationBar } from "@/components/downloads/DestinationBar";
import { EngineNotice } from "@/components/downloads/EngineNotice";
import { JobCard } from "@/components/downloads/JobCard";
import { UrlBar } from "@/components/downloads/UrlBar";
import { useToast } from "@/components/ui/Toast";
import { DownloadIcon, GlobeIcon, ShieldIcon } from "@/components/ui/icons";

export function DownloadsPage() {
  const toast = useToast();
  const [engine, setEngine] = useState<EngineStatus | null>(null);
  const [rechecking, setRechecking] = useState(false);
  const [jobs, setJobs] = useState<JobView[]>([]);
  const [dest, setDest] = useState<Destination | null>(null);
  // A folder the user has browsed to but not yet committed.
  const [pendingDest, setPendingDest] = useState<string | null>(null);
  const [destBusy, setDestBusy] = useState(false);
  const [submitting, setSubmitting] = useState(false);

  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

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
        const [status, list, destination] = await Promise.all([
          downloadEngineStatus(),
          downloadList(),
          downloadGetDestination(),
        ]);
        if (!mounted.current) return;
        setEngine(status);
        setJobs(list);
        setDest(destination);
      } catch {
        if (mounted.current) {
          setEngine({ available: false, path: null, version: null });
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
      onFinished: (job) => {
        upsert(job);
        toast("success", `${job.title ?? "Video"} saved.`);
      },
      onFailed: (job) => {
        upsert(job);
        toast(
          "error",
          downloadMessage(job.error_code, job.error_message ?? "Download failed."),
        );
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
   * Queue a whole paste.
   *
   * One bad link must not discard the good ones, so each is enqueued
   * independently and the failures are summarised in a single toast rather
   * than firing one per URL.
   */
  const start = useCallback(
    async (urls: string[]) => {
      setSubmitting(true);
      const failures: string[] = [];
      try {
        for (const url of urls) {
          try {
            upsert(await downloadStart(url));
          } catch (e) {
            const err = toAuthError(e);
            failures.push(downloadMessage(err.code, err.message));
            // An engine that vanished mid-session should flip the panel back,
            // and there's no point trying the rest of the list without it.
            if (err.code === "engine_missing") {
              void refreshEngine();
              break;
            }
          }
        }
      } finally {
        if (mounted.current) setSubmitting(false);
      }

      if (failures.length === 0) return;
      const queued = urls.length - failures.length;
      // Identical errors collapse: ten non-Facebook links are one complaint.
      const unique = [...new Set(failures)];
      toast(
        "error",
        queued > 0
          ? `${queued} queued, ${failures.length} skipped — ${unique.join(" ")}`
          : unique.join(" "),
      );
    },
    [upsert, toast, refreshEngine],
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

  const clearFinished = useCallback(async () => {
    await downloadClearFinished();
    setJobs((prev) => prev.filter((j) => !isTerminal(j.status)));
  }, []);

  const recheck = useCallback(async () => {
    setRechecking(true);
    try {
      const status = await refreshEngine();
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

  const finishedCount = jobs.filter((j) => isTerminal(j.status)).length;
  const engineReady = engine?.available === true;

  return (
    <div className="page">
      <header className="page__header rise">
        <span className="page__eyebrow">
          <DownloadIcon size={12} />
          Downloads
        </span>
        <h1 className="page__title">Download public videos</h1>
        <p className="page__lede">
          Paste a link — or a whole list, one per line — to public Facebook or
          TikTok videos and reels. No account needed: these are posts anyone
          can already open in a browser.
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

      <div className="rise" style={{ animationDelay: "60ms" }}>
        <UrlBar
          onSubmit={(urls) => void start(urls)}
          busy={submitting}
          disabled={!engineReady}
        />
      </div>

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

        {jobs.length === 0 ? (
          <EmptyQueue engineReady={engineReady} />
        ) : (
          <div className="stack">
            {jobs.map((job, i) => (
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
          ? "Paste a link above — or several at once, one per line. Facebook watch pages, reels and share links, TikTok videos and short links all work."
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
          "Downloads run with no session: no cookies, no browser profile, no access token is ever passed to the engine.",
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
