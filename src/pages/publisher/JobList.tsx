import { useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/Button";
import { useToast } from "@/components/ui/Toast";
import { PlatformMark } from "@/components/publish/PlatformMark";
import { StatusBadge } from "@/components/publish/StatusDot";
import {
  isJobActive,
  JOB_STATUS_LABEL,
  JOB_STATUS_TONE,
  publishCancel,
  publishRemoveJob,
  publishRetry,
  type PublishJob,
} from "@/lib/publish";
import { usePublish } from "@/components/publish/PublishProvider";

/**
 * The queue, rendered the same way everywhere it appears.
 *
 * A "Needs you" job is styled as information, not as an error. It is the
 * expected end state of the current connectors — the app is open with the
 * video attached and the person taps Post — and painting it red would train
 * people to ignore the jobs that genuinely failed.
 */
export function JobList({ jobs }: { jobs: PublishJob[] }) {
  if (jobs.length === 0) {
    return (
      <div className="empty">
        <div className="empty__title">Nothing in the queue</div>
        <div className="empty__text">Published jobs will show up here.</div>
      </div>
    );
  }
  return (
    <div className="joblist">
      {jobs.map((job) => (
        <JobCard key={job.id} job={job} />
      ))}
    </div>
  );
}

function JobCard({ job }: { job: PublishJob }) {
  const toast = useToast();
  const { refreshJobs } = usePublish();
  const [busy, setBusy] = useState(false);
  const [showShot, setShowShot] = useState(false);

  const active = isJobActive(job.status);

  const act = async (label: string, run: () => Promise<unknown>) => {
    setBusy(true);
    try {
      await run();
      await refreshJobs();
    } catch (e) {
      toast("error", `${label} failed: ${message(e)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <article className={`job job--${job.status}`}>
      <PlatformMark platform={job.platform} />

      <div className="job__text">
        <div className="job__head">
          <span className="job__account">{job.account_name}</span>
          <StatusBadge tone={JOB_STATUS_TONE[job.status]} pulse={active}>
            {JOB_STATUS_LABEL[job.status]}
          </StatusBadge>
        </div>
        <div className="job__media">{job.media_name}</div>

        {active && (
          <div className="job__bar" role="progressbar" aria-valuenow={Math.round(job.progress * 100)}>
            <span style={{ width: `${Math.max(3, job.progress * 100)}%` }} />
          </div>
        )}

        {job.step && <div className="job__step">{job.step}</div>}

        {/* `needs_user` is guidance, not a failure, so it reads as a note. */}
        {job.error && (
          <div
            className={`job__msg ${
              job.error_code === "needs_user" ? "job__msg--info" : "job__msg--error"
            }`}
          >
            {job.error}
          </div>
        )}

        {job.screenshot_path && (
          <div className="job__shot">
            <button className="linkbtn" type="button" onClick={() => setShowShot((s) => !s)}>
              {showShot ? "Hide screenshot" : "Show screenshot"}
            </button>
            {showShot && (
              <img
                className="job__shotimg"
                src={convertFileSrc(job.screenshot_path)}
                alt={`Emulator screen for ${job.account_name}`}
              />
            )}
          </div>
        )}
      </div>

      <div className="job__actions">
        {active ? (
          <Button variant="ghost" loading={busy} onClick={() => act("Cancel", () => publishCancel(job.id))}>
            Cancel
          </Button>
        ) : (
          <>
            {job.status !== "published" && (
              <Button variant="ghost" loading={busy} onClick={() => act("Retry", () => publishRetry(job.id))}>
                Retry
              </Button>
            )}
            <Button
              variant="ghost"
              loading={busy}
              onClick={() => act("Remove", () => publishRemoveJob(job.id))}
            >
              Remove
            </Button>
          </>
        )}
      </div>
    </article>
  );
}

function message(e: unknown): string {
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}
