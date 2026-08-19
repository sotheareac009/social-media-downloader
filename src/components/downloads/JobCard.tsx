import {
  downloadMessage,
  formatBytes,
  formatDuration,
  formatEta,
  formatSpeed,
  isTerminal,
  type JobView,
} from "@/lib/download";
import {
  AlertIcon,
  CheckIcon,
  ClockIcon,
  FolderIcon,
  StopIcon,
  TrashIcon,
  XIcon,
} from "@/components/ui/icons";

const SOURCE_LABEL: Record<JobView["source"], string> = {
  facebook: "Facebook",
  tiktok: "TikTok",
  youtube: "YouTube",
  instagram: "Instagram",
};

export function JobCard({
  job,
  onCancel,
  onRemove,
  onReveal,
}: {
  job: JobView;
  onCancel: () => void;
  onRemove: () => void;
  onReveal: () => void;
}) {
  const running = !isTerminal(job.status);
  const pct = job.fraction === null ? null : Math.round(job.fraction * 100);

  return (
    <article className={`job job--${job.status} job--${job.source}`}>
      <div className="job__thumb">
        {job.thumbnail_url ? (
          // Remote https images are permitted by the CSP; a broken one falls
          // back to the source glyph rather than an empty box.
          <img
            src={job.thumbnail_url}
            alt=""
            loading="lazy"
            onError={(e) => {
              e.currentTarget.style.display = "none";
            }}
          />
        ) : null}
        <span className={`job__badge logo--${job.source}`}>
          {SOURCE_LABEL[job.source].slice(0, 2)}
        </span>
      </div>

      <div className="job__body">
        <div className="job__title" title={job.title ?? job.url}>
          {job.title ?? job.url}
        </div>

        <div className="job__meta">
          <span className="job__source">{SOURCE_LABEL[job.source]}</span>
          {job.uploader && <span>{job.uploader}</span>}
          {job.duration_seconds !== null && (
            <span>{formatDuration(job.duration_seconds)}</span>
          )}
        </div>

        {running && <ProgressRow job={job} pct={pct} />}

        {job.status === "completed" && (
          <div
            className={`job__status ${
              job.audio_only && !job.still_image_video
                ? "job__status--note"
                : "job__status--ok"
            }`}
          >
            <CheckIcon size={13} />
            {job.still_image_video ? (
              <span>
                Saved as video · {formatBytes(job.downloaded_bytes || job.total_bytes)}
                <span className="job__why">
                  {" "}
                  — photo post, built from its cover image and audio
                </span>
              </span>
            ) : job.audio_only ? (
              <span>
                Saved audio only · {formatBytes(job.downloaded_bytes || job.total_bytes)}
                <span className="job__why">
                  {" "}
                  — this post is a photo slideshow, so it has no video track
                </span>
              </span>
            ) : job.converted_from ? (
              <span>
                Saved · {formatBytes(job.downloaded_bytes || job.total_bytes)}
                <span className="job__why">
                  {" "}
                  — converted from {job.converted_from} so it plays in QuickTime
                </span>
              </span>
            ) : (
              <>Saved · {formatBytes(job.downloaded_bytes || job.total_bytes)}</>
            )}
          </div>
        )}

        {job.status === "cancelled" && (
          <div className="job__status job__status--muted">
            <XIcon size={13} />
            Cancelled
          </div>
        )}

        {job.status === "failed" && (
          <div className="job__status job__status--bad">
            <AlertIcon size={13} />
            {downloadMessage(job.error_code, job.error_message ?? "Download failed.", job.source)}
          </div>
        )}
      </div>

      <div className="job__actions">
        {running ? (
          <button
            className="btn btn--ghost btn--sm"
            type="button"
            onClick={onCancel}
          >
            <StopIcon size={13} />
            Cancel
          </button>
        ) : (
          <>
            {job.status === "completed" && job.output_path && (
              <button
                className="btn btn--ghost btn--sm"
                type="button"
                onClick={onReveal}
              >
                <FolderIcon size={13} />
                Show
              </button>
            )}
            <button
              className="btn btn--ghost btn--sm"
              type="button"
              onClick={onRemove}
              aria-label="Remove from list"
              title="Remove from list"
            >
              <TrashIcon size={13} />
            </button>
          </>
        )}
      </div>
    </article>
  );
}

function ProgressRow({ job, pct }: { job: JobView; pct: number | null }) {
  // Before the first byte there is nothing to measure, so the bar runs as an
  // indeterminate stripe rather than sitting frozen at zero.
  const indeterminate = pct === null;

  return (
    <>
      <div
        className={`progress ${indeterminate ? "progress--indeterminate" : ""}`}
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={pct ?? undefined}
      >
        <div
          className="progress__fill"
          style={indeterminate ? undefined : { width: `${pct}%` }}
        />
      </div>
      <div className="job__stats">
        <span className="job__phase">
          {job.status === "queued" && (
            <>
              <ClockIcon size={12} />
              {/* A backoff is a wait, not a hang — say which attempt we're on. */}
              {job.attempt > 1
                ? `Rate-limited, retrying ${job.attempt} of ${job.max_attempts}…`
                : "Queued"}
            </>
          )}
          {job.status === "probing" &&
            (job.attempt > 1
              ? `Retrying ${job.attempt} of ${job.max_attempts}…`
              : "Reading link…")}
          {job.status === "downloading" &&
            (pct === null ? "Starting…" : `${pct}%`)}
        </span>
        {job.status === "downloading" && (
          <>
            <span>
              {formatBytes(job.downloaded_bytes)}
              {job.total_bytes ? ` / ${formatBytes(job.total_bytes)}` : ""}
            </span>
            {job.speed_bps ? <span>{formatSpeed(job.speed_bps)}</span> : null}
            {job.eta_seconds ? <span>{formatEta(job.eta_seconds)}</span> : null}
          </>
        )}
      </div>
    </>
  );
}
