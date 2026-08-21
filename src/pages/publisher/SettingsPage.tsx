import { useEffect, useState } from "react";
import { Button } from "@/components/ui/Button";
import { useToast } from "@/components/ui/Toast";
import { usePublish } from "@/components/publish/PublishProvider";
import {
  ldplayerBrowsePath,
  ldplayerGetSettings,
  ldplayerRedetect,
  ldplayerSetSettings,
  type DeviceSettings,
} from "@/lib/ldplayer";
import { FolderIcon, TerminalIcon, TrashIcon } from "@/components/ui/icons";

/** Matches the clamp the Rust settings apply, so the UI can't offer an invalid value. */
const CONCURRENCY = [1, 2, 3, 4, 5, 6, 7, 8];

export function PublisherSettingsPage() {
  const toast = useToast();
  const { environment, logs, clearLogs, refreshEnvironment, refreshDevices } = usePublish();
  const [settings, setSettings] = useState<DeviceSettings | null>(null);
  const [saving, setSaving] = useState(false);
  const [detecting, setDetecting] = useState(false);

  useEffect(() => {
    ldplayerGetSettings().then(setSettings).catch(() => setSettings(null));
  }, []);

  // Save on every change rather than behind a button: each field is a single
  // value with an immediate, visible effect (paths re-run detection), and a
  // Save button here would just be a way to lose a change by navigating away.
  const update = async (patch: Partial<DeviceSettings>) => {
    if (!settings) return;
    const next = { ...settings, ...patch };
    setSettings(next);
    setSaving(true);
    try {
      const saved = await ldplayerSetSettings(next);
      setSettings(saved);
      await refreshEnvironment();
    } catch (e) {
      toast("error", `Could not save: ${message(e)}`);
    } finally {
      setSaving(false);
    }
  };

  const browse = async (field: "ldplayer_path" | "adb_path") => {
    try {
      const picked = await ldplayerBrowsePath(field === "ldplayer_path" ? "folder" : "file");
      if (picked) await update({ [field]: picked } as Partial<DeviceSettings>);
    } catch (e) {
      toast("error", `Could not open the picker: ${message(e)}`);
    }
  };

  const redetect = async () => {
    setDetecting(true);
    try {
      await ldplayerRedetect();
      await refreshEnvironment();
      await refreshDevices();
      toast("success", "Re-checked for LDPlayer and ADB.");
    } catch (e) {
      toast("error", message(e));
    } finally {
      setDetecting(false);
    }
  };

  if (!settings) return null;

  return (
    <div className="page">
      <header className="page__head">
        <div>
          <h1 className="page__title">Publishing settings</h1>
          <p className="page__lede">
            Where LDPlayer and ADB live, and how the queue behaves.
          </p>
        </div>
        <Button variant="ghost" loading={detecting} onClick={() => void redetect()}>
          Re-detect
        </Button>
      </header>

      <section className="section">
        <h2 className="section__title">Tools</h2>

        <div className="setrow">
          <div className="setrow__text">
            <div className="setrow__label">LDPlayer folder</div>
            <div className="setrow__hint">
              The folder containing <code>ldconsole.exe</code>. Leave empty to detect it
              automatically.
            </div>
            <div className="setrow__value">
              {environment?.ldplayer_path ?? "Not found"}
            </div>
          </div>
          <div className="setrow__actions">
            <Button variant="ghost" icon={<FolderIcon size={14} />} onClick={() => void browse("ldplayer_path")}>
              Choose
            </Button>
            {settings.ldplayer_path && (
              <Button
                variant="ghost"
                icon={<TrashIcon size={14} />}
                onClick={() => void update({ ldplayer_path: null })}
                aria-label="Clear the LDPlayer path"
              />
            )}
          </div>
        </div>

        <div className="setrow">
          <div className="setrow__text">
            <div className="setrow__label">ADB executable</div>
            <div className="setrow__hint">
              Leave empty to use the copy bundled with LDPlayer. Mixing ADB versions can
              knock running instances offline, so the bundled one is preferred.
            </div>
            <div className="setrow__value">
              {environment?.adb_path ?? "Not found"}
              {environment?.adb_version ? ` · ${environment.adb_version}` : ""}
            </div>
          </div>
          <div className="setrow__actions">
            <Button variant="ghost" icon={<TerminalIcon size={14} />} onClick={() => void browse("adb_path")}>
              Choose
            </Button>
            {settings.adb_path && (
              <Button
                variant="ghost"
                icon={<TrashIcon size={14} />}
                onClick={() => void update({ adb_path: null })}
                aria-label="Clear the ADB path"
              />
            )}
          </div>
        </div>
      </section>

      <section className="section">
        <h2 className="section__title">Publishing</h2>

        <div className="setrow">
          <div className="setrow__text">
            <div className="setrow__label">Upload folder on the device</div>
            <div className="setrow__hint">
              Where videos are copied inside Android. <code>/sdcard/Movies/…</code> is
              indexed as video by Android's gallery, which is what the social apps read.
            </div>
          </div>
          <input
            className="input"
            value={settings.remote_dir}
            onChange={(e) => setSettings({ ...settings, remote_dir: e.target.value })}
            onBlur={(e) => void update({ remote_dir: e.target.value })}
            spellCheck={false}
          />
        </div>

        <div className="setrow">
          <div className="setrow__text">
            <div className="setrow__label">Maximum concurrent jobs</div>
            <div className="setrow__hint">
              Emulators share one CPU and one disk. More than two or three usually makes
              every job slower and can make devices drop offline.
            </div>
          </div>
          <select
            className="input input--sm"
            value={settings.max_concurrent}
            onChange={(e) => void update({ max_concurrent: Number(e.target.value) })}
          >
            {CONCURRENCY.map((n) => (
              <option key={n} value={n}>
                {n}
              </option>
            ))}
          </select>
        </div>

        <Toggle
          label="Delete the video from the device after publishing"
          hint="Off by default: a failed job's file is the thing you want to inspect."
          checked={settings.cleanup_after_publish}
          onChange={(v) => void update({ cleanup_after_publish: v })}
        />

        <Toggle
          label="Verbose logging and step screenshots"
          hint="Records every ADB command and captures the emulator screen at each step. Useful when diagnosing a connector; noisy otherwise."
          checked={settings.verbose_logging}
          onChange={(v) => void update({ verbose_logging: v })}
        />
      </section>

      <section className="section">
        <div className="section__head">
          <h2 className="section__title">Log</h2>
          {logs.length > 0 && (
            <button className="linkbtn" type="button" onClick={clearLogs}>
              Clear
            </button>
          )}
        </div>
        {logs.length === 0 ? (
          <div className="empty">
            <div className="empty__title">Nothing logged yet</div>
            <div className="empty__text">
              Turn on verbose logging above to see every step as it runs. Errors are
              always logged.
            </div>
          </div>
        ) : (
          <div className="logpane">
            {logs.map((line, i) => (
              <div key={`${line.at}-${i}`} className={`logline logline--${line.level}`}>
                <span className="logline__scope">{line.scope ?? "—"}</span>
                <span className="logline__msg">{line.message}</span>
              </div>
            ))}
          </div>
        )}
      </section>

      {saving && <div className="savedhint">Saving…</div>}
    </div>
  );
}

function Toggle({
  label,
  hint,
  checked,
  onChange,
}: {
  label: string;
  hint: string;
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <label className="setrow setrow--toggle">
      <div className="setrow__text">
        <div className="setrow__label">{label}</div>
        <div className="setrow__hint">{hint}</div>
      </div>
      <input
        type="checkbox"
        className="switch"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
      />
    </label>
  );
}

function message(e: unknown): string {
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}
