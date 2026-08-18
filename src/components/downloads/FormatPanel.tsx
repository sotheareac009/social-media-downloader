import { Button } from "@/components/ui/Button";
import { DownloadIcon, XIcon } from "@/components/ui/icons";
import { formatDuration, type FormatReport, type Quality } from "@/lib/download";

/**
 * What one link actually offers.
 *
 * The point is honesty about a specific video rather than a fixed ladder: a
 * video that has 8K offers 8K here, and one that tops out at 720p doesn't
 * pretend otherwise. Tiers come from the platform's own labelling — an
 * ultrawide 8K stream is 7680x3200, so its *height* is 3200 and only its label
 * says 4320p.
 */
export function FormatPanel({
  report,
  chosen,
  busy,
  hasFfmpeg,
  onChoose,
  onDownload,
  onDismiss,
}: {
  report: FormatReport;
  chosen: Quality;
  busy: boolean;
  hasFfmpeg: boolean;
  onChoose: (q: Quality) => void;
  onDownload: () => void;
  onDismiss: () => void;
}) {
  const { info, formats, best_label } = report;

  return (
    <article className="fmt">
      <div className="fmt__head">
        {info.thumbnail_url && (
          <img className="fmt__thumb" src={info.thumbnail_url} alt="" loading="lazy" />
        )}
        <div className="fmt__ident">
          <div className="fmt__title" title={info.title}>
            {info.title}
          </div>
          <div className="fmt__meta">
            {info.uploader && <span>{info.uploader}</span>}
            {info.duration_seconds !== null && (
              <span>{formatDuration(info.duration_seconds)}</span>
            )}
            {best_label && <span>up to {best_label}</span>}
          </div>
        </div>
        <button
          className="btn btn--ghost btn--sm"
          type="button"
          onClick={onDismiss}
          aria-label="Dismiss"
          title="Dismiss"
        >
          <XIcon size={13} />
        </button>
      </div>

      <div className="fmt__choices" role="group" aria-label="Quality">
        <Chip
          label="Best"
          active={chosen === "best"}
          onClick={() => onChoose("best")}
        />
        {formats.map((f) => {
          const q = `${f.tier}p` as Quality;
          return (
            <Chip
              key={f.tier}
              label={f.label}
              sub={f.width && f.height ? `${f.width}×${f.height}` : undefined}
              active={chosen === q}
              onClick={() => onChoose(q)}
            />
          );
        })}
      </div>

      {!hasFfmpeg && (
        <p className="fmt__warn">
          Without FFmpeg only the single-file streams are usable, so this will
          download at 360p whichever tier you pick.
        </p>
      )}

      <div className="fmt__foot">
        <Button loading={busy} onClick={onDownload} icon={<DownloadIcon size={14} />}>
          Download
        </Button>
      </div>
    </article>
  );
}

function Chip({
  label,
  sub,
  active,
  onClick,
}: {
  label: string;
  sub?: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={`chip ${active ? "chip--active" : ""}`.trim()}
      onClick={onClick}
      aria-pressed={active}
    >
      <span className="chip__label">{label}</span>
      {sub && <span className="chip__sub">{sub}</span>}
    </button>
  );
}
