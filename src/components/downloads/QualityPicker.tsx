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
}: {
  settings: QualitySettings;
  busy: boolean;
  onChange: (q: Quality) => void;
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
    </div>
  );
}
