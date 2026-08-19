import type { QualitySettings, Quality } from "@/lib/download";

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
}: {
  settings: QualitySettings;
  busy: boolean;
  onChange: (q: Quality) => void;
  onToggleCompatible: (on: boolean) => void;
}) {
  const selected = settings.options.find((o) => o.id === settings.selected);
  const capped = !settings.has_ffmpeg && selected?.needs_ffmpeg;

  return (
    <div className="quality">
      <label className="quality__label" htmlFor="quality-select">
        Quality
      </label>
      <select
        id="quality-select"
        className="quality__select"
        value={settings.selected}
        disabled={busy}
        onChange={(e) => onChange(e.target.value as Quality)}
      >
        {settings.options.map((o) => (
          <option key={o.id} value={o.id}>
            {o.label}
            {!settings.has_ffmpeg && o.needs_ffmpeg ? " — needs FFmpeg" : ""}
          </option>
        ))}
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
