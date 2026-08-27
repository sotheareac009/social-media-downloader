import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useToast } from "@/components/ui/Toast";
import {
  ldplayerListDevices,
  ldplayerTransferMedia,
  type DeviceView,
} from "@/lib/ldplayer";

/**
 * Choosing devices to copy finished files to, shared by Split and Merge.
 *
 * Both screens end the same way — files on disk that someone wants on a phone
 * or emulator — so the awkward parts live here once: pruning a device that
 * went offline, counting copies rather than files when several are selected,
 * and letting one unplugged device fail without costing the others theirs.
 */
export interface DeviceTargets {
  devices: DeviceView[] | null;
  online: DeviceView[];
  selected: Set<string>;
  setSelected: (next: Set<string>) => void;
  refresh: () => void;
  /** Non-null while copying: `done` of `total` individual copies. */
  sending: { done: number; total: number } | null;
  /** Copy every path to every selected device. Resolves when all are done. */
  send: (paths: string[]) => Promise<void>;
}

export function useDeviceTargets(active: boolean): DeviceTargets {
  const toast = useToast();
  const [devices, setDevices] = useState<DeviceView[] | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [sending, setSending] = useState<{ done: number; total: number } | null>(null);

  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const refresh = useCallback(() => {
    void ldplayerListDevices()
      .then((found) => {
        if (!mounted.current) return;
        setDevices(found);
        // A device that went away must not stay selected, or the copy fails at
        // the end of a job that otherwise worked.
        const online = new Set(
          found.filter((d) => d.state === "online").map((d) => d.id),
        );
        setSelected((prev) => new Set([...prev].filter((id) => online.has(id))));
      })
      // No adb, no devices — the picker says so rather than erroring.
      .catch(() => mounted.current && setDevices([]));
  }, []);

  useEffect(() => {
    // Listing starts an adb server; not something to do for a hidden tab.
    if (active) refresh();
  }, [active, refresh]);

  /** Only a booted device can take a file; the rest are noise in a picker. */
  const online = useMemo(
    () => (devices ?? []).filter((d) => d.state === "online"),
    [devices],
  );

  const send = useCallback(
    async (paths: string[]) => {
      const targets = [...selected];
      if (targets.length === 0 || paths.length === 0) return;

      // Copies, not files: "7 of 12" means something with two devices chosen.
      const total = targets.length * paths.length;
      setSending({ done: 0, total });
      let sent = 0;
      let failed = 0;
      let step = 0;

      for (const id of targets) {
        for (const path of paths) {
          try {
            // The publisher's own transfer: it pushes *and* tells MediaStore
            // the file exists. A plain push lands a file no gallery can see.
            await ldplayerTransferMedia(id, path);
            sent++;
          } catch {
            failed++;
          }
          step++;
          if (!mounted.current) return;
          setSending({ done: step, total });
        }
      }

      if (!mounted.current) return;
      setSending(null);
      const where =
        targets.length === 1
          ? (devices?.find((d) => d.id === targets[0])?.name ?? "the device")
          : `${targets.length} devices`;
      toast(
        failed === 0 ? "success" : "error",
        failed === 0
          ? `Copied ${sent} file${sent === 1 ? "" : "s"} to ${where}.`
          : `Copied ${sent}, ${failed} failed reaching ${where}.`,
      );
    },
    [selected, devices, toast],
  );

  return { devices, online, selected, setSelected, refresh, sending, send };
}

/**
 * The picker itself, in the shape the Publish screen uses — tick-box cards
 * rather than a dropdown, because choosing three of five devices from a select
 * is a fight, and this is the same choice being made there.
 */
export function DevicePicker({
  targets,
  disabled = false,
  idleNote,
}: {
  targets: DeviceTargets;
  disabled?: boolean;
  /** Shown when nothing is selected — what happens if you pick none. */
  idleNote: string;
}) {
  const { devices, online, selected, setSelected, refresh, sending } = targets;
  const busy = disabled || sending !== null;

  return (
    <div className="devicepick">
      <div className="devicepick__head">
        <span className="outdir__label">Also copy to a device</span>
        <div className="devicepick__actions">
          {online.length > 1 && (
            <button
              className="btn btn--ghost btn--sm"
              type="button"
              disabled={busy}
              onClick={() =>
                setSelected(
                  selected.size === online.length
                    ? new Set()
                    : new Set(online.map((d) => d.id)),
                )
              }
            >
              {selected.size === online.length ? "Clear these" : "Select these"}
            </button>
          )}
          <button
            className="btn btn--ghost btn--sm"
            type="button"
            disabled={busy}
            onClick={refresh}
            title="Look for devices again"
          >
            Refresh
          </button>
        </div>
      </div>

      {online.length === 0 ? (
        <p className="devicepick__none">
          {devices === null
            ? "Looking for devices…"
            : "No running device found. Start an emulator or plug a phone in, then Refresh."}
        </p>
      ) : (
        <div className="pickgrid">
          {online.map((device) => {
            const on = selected.has(device.id);
            return (
              <button
                key={device.id}
                type="button"
                className={`pick ${on ? "pick--on" : ""}`.trim()}
                aria-pressed={on}
                disabled={busy}
                onClick={() => {
                  const next = new Set(selected);
                  if (next.has(device.id)) next.delete(device.id);
                  else next.add(device.id);
                  setSelected(next);
                }}
              >
                <span className="pick__box" aria-hidden>
                  {on ? "✓" : ""}
                </span>
                <span className="pick__text">
                  <span className="pick__name">{device.name}</span>
                  <span className="pick__meta">
                    {device.model ?? device.serial ?? device.id}
                    {device.android_release ? ` · Android ${device.android_release}` : ""}
                  </span>
                </span>
              </button>
            );
          })}
        </div>
      )}

      <p className="devicepick__note">
        {sending
          ? `Copying ${sending.done} of ${sending.total}…`
          : selected.size > 0
            ? `Lands in the gallery of ${selected.size} device${selected.size === 1 ? "" : "s"}, ready to post from there.`
            : idleNote}
      </p>
    </div>
  );
}
