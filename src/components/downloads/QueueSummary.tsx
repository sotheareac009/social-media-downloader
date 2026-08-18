import { Button } from "@/components/ui/Button";
import { AlertIcon, CheckIcon, ClockIcon, XIcon } from "@/components/ui/icons";
import { formatBytes, type JobView } from "@/lib/download";

export type QueueFilter = "all" | "active" | "completed" | "failed";

export interface QueueCounts {
  total: number;
  active: number;
  completed: number;
  failed: number;
  cancelled: number;
  bytes: number;
}

/** Derive every number the summary shows in one pass. */
export function countJobs(jobs: JobView[]): QueueCounts {
  const c: QueueCounts = {
    total: jobs.length,
    active: 0,
    completed: 0,
    failed: 0,
    cancelled: 0,
    bytes: 0,
  };
  for (const j of jobs) {
    if (j.status === "completed") {
      c.completed++;
      c.bytes += j.downloaded_bytes;
    } else if (j.status === "failed") c.failed++;
    else if (j.status === "cancelled") c.cancelled++;
    else c.active++;
  }
  return c;
}

/**
 * Batch outcome at a glance.
 *
 * With a queue of one this is noise, so it only appears once there's more than
 * one job — the point is answering "how did the 133 go?" without scrolling
 * through 133 rows.
 */
export function QueueSummary({
  counts,
  filter,
  onFilter,
  onRetryFailed,
  retrying,
  armed,
  onDisarm,
}: {
  counts: QueueCounts;
  filter: QueueFilter;
  onFilter: (f: QueueFilter) => void;
  onRetryFailed: () => void;
  retrying: boolean;
  /** Waiting for the queue to drain before retrying. */
  armed: boolean;
  onDisarm: () => void;
}) {
  const done = counts.completed + counts.failed + counts.cancelled;
  const finished = counts.active === 0 && counts.total > 0;

  return (
    <section className="summary">
      <div className="summary__row">
        <Stat
          label={counts.active > 0 ? "In progress" : "Queued"}
          value={counts.active}
          tone="neutral"
          icon={<ClockIcon size={13} />}
          active={filter === "active"}
          onClick={() => onFilter(filter === "active" ? "all" : "active")}
        />
        <Stat
          label="Downloaded"
          value={counts.completed}
          tone="ok"
          icon={<CheckIcon size={13} />}
          active={filter === "completed"}
          onClick={() => onFilter(filter === "completed" ? "all" : "completed")}
        />
        <Stat
          label="Failed"
          value={counts.failed}
          tone="bad"
          icon={<AlertIcon size={13} />}
          active={filter === "failed"}
          onClick={() => onFilter(filter === "failed" ? "all" : "failed")}
        />
        {counts.cancelled > 0 && (
          <Stat
            label="Cancelled"
            value={counts.cancelled}
            tone="muted"
            icon={<XIcon size={13} />}
            active={false}
            onClick={() => onFilter("all")}
          />
        )}
      </div>

      {/* Batch progress by job count, not bytes: totals aren't known until
          each video is probed, so a byte-based bar would jump around. */}
      {counts.active > 0 && (
        <div className="progress summary__bar">
          <div
            className="progress__fill"
            style={{ width: `${Math.round((done / counts.total) * 100)}%` }}
          />
        </div>
      )}

      <div className="summary__foot">
        <span>
          {finished
            ? `Finished — ${counts.completed} of ${counts.total} saved`
            : `${done} of ${counts.total} done`}
          {counts.completed > 0 && ` · ${formatBytes(counts.bytes)}`}
        </span>
        {counts.failed > 0 && !armed && (
          <Button
            variant="ghost"
            className="btn--sm"
            loading={retrying}
            onClick={onRetryFailed}
          >
            {/* Retrying while downloads are still running just adds contention
                on the platform that rejected them, so the button offers to
                wait instead. */}
            {counts.active > 0
              ? `Retry ${counts.failed} failed when finished`
              : `Retry all ${counts.failed} failed`}
          </Button>
        )}
        {armed && (
          <span className="summary__armed">
            <ClockIcon size={12} />
            Will retry {counts.failed} once the queue finishes
            <button type="button" className="summary__cancel" onClick={onDisarm}>
              Cancel
            </button>
          </span>
        )}
      </div>
    </section>
  );
}

function Stat({
  label,
  value,
  tone,
  icon,
  active,
  onClick,
}: {
  label: string;
  value: number;
  tone: "neutral" | "ok" | "bad" | "muted";
  icon: React.ReactNode;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={`stat stat--${tone} ${active ? "stat--active" : ""}`.trim()}
      onClick={onClick}
      // Zero of something is still worth showing, but not worth filtering to.
      disabled={value === 0}
      aria-pressed={active}
      title={value === 0 ? undefined : `Show only these`}
    >
      <span className="stat__icon">{icon}</span>
      <span className="stat__value">{value}</span>
      <span className="stat__label">{label}</span>
    </button>
  );
}
