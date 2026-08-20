import type { QualitySettings, Quality, VideoFormat } from "@/lib/download";

/**
 * Quality preference.
 *
 * Options that need FFmpeg stay selectable when it's missing, but say so:
 * disabling them would leave someone staring at a greyed-out 1080p with no
 * explanation of why. Choosing one without FFmpeg still works — it just falls
 * back to the best single file, which on YouTube is 360p.
 */
export function QualityPicker({
  settings,
  busy,
  onChange,
  onToggleCompatible,
  formats,
  picked,
  onPick,
  bestLabel,
}: {
  settings: QualitySettings;
  busy: boolean;
  onChange: (q: Quality) => void;
  onToggleCompatible: (on: boolean) => void;
  /** A validated single video's real tiers. When present, the dropdown offers
   *  exactly these instead of the generic ladder. */
  formats?: VideoFormat[];
  picked?: Quality;
  onPick?: (q: Quality) => void;
  bestLabel?: string | null;
}) {
  // When a single video has been inspected, drive the dropdown from its real
  // tiers (controlled by the transient picked-quality) instead of the global
  // preference — so there is one place to choose, not a duplicate card.
  const useVideo = !!(formats && formats.length > 0);
  const selectValue = useVideo ? picked ?? "best" : settings.selected;
  const handleChange = (q: Quality) => (useVideo ? onPick?.(q) : onChange(q));

  const globalSel = settings.options.find((o) => o.id === settings.selected);
  const pickedTier = typeof selectValue === "string" && selectValue.endsWith("p")
    ? parseInt(selectValue, 10)
    : 0;
  const capped = useVideo
    ? !settings.has_ffmpeg && pickedTier > 360
    : !settings.has_ffmpeg && globalSel?.needs_ffmpeg;

  return (
    <div className="quality">
      <label className="quality__label" htmlFor="quality-select">
        Quality
      </label>
      <select
        id="quality-select"
        className="quality__select"
        value={selectValue}
        disabled={busy}
        onChange={(e) => handleChange(e.target.value as Quality)}
      >
        {useVideo ? (
          <>
            <option value="best">
              Best available{bestLabel ? ` (up to ${bestLabel})` : ""}
            </option>
            {formats!.map((f) => (
              <option key={f.tier} value={`${f.tier}p`}>
                {f.label}
                {f.width && f.height ? ` — ${f.width}×${f.height}` : ""}
              </option>
            ))}
          </>
        ) : (
          settings.options.map((o) => (
            <option key={o.id} value={o.id}>
              {o.label}
              {!settings.has_ffmpeg && o.needs_ffmpeg ? " — needs FFmpeg" : ""}
            </option>
          ))
        )}
      </select>
      {capped && (
        <span className="quality__warn" title="Install FFmpeg to lift the cap">
          YouTube will still give 360p
        </span>
      )}

      {/* QuickTime cannot decode VP9 or AV1, so without this a finished
          download can be a file macOS refuses to open. */}
      <label className="quality__compat" title="H.264 plays in QuickTime, Photos and iOS. Turning this off allows 4K and 8K, which are only offered as VP9 or AV1.">
        <input
          type="checkbox"
          checked={settings.prefer_compatible}
          disabled={busy}
          onChange={(e) => onToggleCompatible(e.target.checked)}
        />
        <span>Playable on Apple devices (H.264)</span>
      </label>
      {settings.prefer_compatible && (
        <span className="quality__hint">caps YouTube at 1080p</span>
      )}
    </div>
  );
}
