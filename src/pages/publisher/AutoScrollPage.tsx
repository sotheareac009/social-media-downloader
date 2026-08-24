import { useEffect, useMemo, useState } from "react";
import { Button } from "@/components/ui/Button";
import { usePublish } from "@/components/publish/PublishProvider";
import { StatusBadge } from "@/components/publish/StatusDot";
import { useToast } from "@/components/ui/Toast";
import {
  DEVICE_STATE_LABEL,
  SCROLL_APPS,
  ldplayerPackages,
  ldplayerAutoscrollStart,
  ldplayerAutoscrollStatus,
  ldplayerAutoscrollStop,
  ldplayerStart,
} from "@/lib/ldplayer";

/**
 * Launch selected LDPlayer instances and auto-scroll their feeds — repeated
 * upward swipes at a chosen interval, across every ticked instance at once.
 */
export function AutoScrollPage() {
  const toast = useToast();
  const { devices, environment, scanning, refreshDevices } = usePublish();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [seconds, setSeconds] = useState(4);
  const [appId, setAppId] = useState<string>(SCROLL_APPS[0].id);
  // Packages installed across the selected (online) devices, to filter the app
  // list. A stopped device can't be queried, so an empty set means "unknown".
  const [installed, setInstalled] = useState<Set<string>>(new Set());
  const [loadingApps, setLoadingApps] = useState(false);
  const [scrolling, setScrolling] = useState(false);
  const [starting, setStarting] = useState(false);

  // Every device can be scrolled (it's just an adb swipe): LDPlayer instances
  // and plain adb devices like an Android Studio emulator alike. Only LDPlayer
  // ones can be *launched* from here — an adb emulator is started elsewhere.
  const instances = devices;

  useEffect(() => {
    let alive = true;
    ldplayerAutoscrollStatus()
      .then((s) => alive && setScrolling(s))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);

  // Drop selections for instances that vanished.
  useEffect(() => {
    setSelected((prev) => new Set([...prev].filter((id) => instances.some((d) => d.id === id))));
  }, [instances]);

  // Read which apps are installed on the selected devices, to filter the picker.
  useEffect(() => {
    const ids = [...selected];
    if (ids.length === 0) {
      setInstalled(new Set());
      return;
    }
    let alive = true;
    setLoadingApps(true);
    void (async () => {
      const union = new Set<string>();
      for (const id of ids) {
        try {
          const pkgs = await ldplayerPackages(id);
          pkgs.forEach((p) => union.add(p));
        } catch {
          /* offline/unreadable device — just contributes nothing */
        }
      }
      if (alive) {
        setInstalled(union);
        setLoadingApps(false);
      }
    })();
    return () => {
      alive = false;
    };
  }, [selected]);

  // Only apps present on the selection. When nothing could be read (no device
  // selected, or a stopped instance), fall back to the full list rather than an
  // empty dropdown the user can't act on.
  const availableApps = useMemo(() => {
    if (installed.size === 0) return SCROLL_APPS;
    return SCROLL_APPS.filter((a) => a.packages.some((p) => installed.has(p)));
  }, [installed]);

  // Keep the selected app valid as the available set changes.
  useEffect(() => {
    if (availableApps.length > 0 && !availableApps.some((a) => a.id === appId)) {
      setAppId(availableApps[0].id);
    }
  }, [availableApps, appId]);

  const toggle = (id: string) =>
    setSelected((prev) => {
      const n = new Set(prev);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });

  const selectedIds = [...selected];
  const chosenApp = SCROLL_APPS.find((a) => a.id === appId);
  const supported = environment?.ldplayer_supported !== false;

  const startInstances = async () => {
    if (selectedIds.length === 0) return;
    setStarting(true);
    let ok = 0;
    for (const id of selectedIds) {
      try {
        await ldplayerStart(id);
        ok += 1;
      } catch {
        /* a failure is per-instance; keep going */
      }
    }
    setStarting(false);
    void refreshDevices();
    toast(ok > 0 ? "success" : "error", `Started ${ok} of ${selectedIds.length} instance(s).`);
  };

  const startScroll = async () => {
    if (selectedIds.length === 0) {
      toast("info", "Pick at least one instance first.");
      return;
    }
    // If an app was chosen, make sure at least one of its package variants is
    // installed on a selected device — otherwise it would silently just scroll
    // the home screen.
    if (chosenApp) {
      let installedSomewhere = false;
      for (const id of selectedIds) {
        try {
          const pkgs = await ldplayerPackages(id);
          if (chosenApp.packages.some((p) => pkgs.includes(p))) {
            installedSomewhere = true;
            break;
          }
        } catch {
          /* if we can't read packages, don't block — let the launch try */
          installedSomewhere = true;
          break;
        }
      }
      if (!installedSomewhere) {
        toast(
          "error",
          `${chosenApp.label} isn't installed on the selected device(s). Install it there, or pick an app that is.`,
        );
        return;
      }
    }
    try {
      await ldplayerAutoscrollStart(
        selectedIds,
        Math.round(seconds * 1000),
        chosenApp?.packages,
      );
      setScrolling(true);
      toast("success", `Auto-scrolling ${selectedIds.length} instance(s).`);
    } catch (e) {
      toast("error", messageOf(e));
    }
  };

  const stopScroll = async () => {
    try {
      await ldplayerAutoscrollStop();
    } finally {
      setScrolling(false);
    }
  };

  return (
    <div className="page">
      <header className="page__head">
        <div>
          <h1 className="page__title">Auto-scroll</h1>
          <p className="page__lede">
            Pick the devices, choose an app, and hit Start auto-scroll. Stopped LDPlayer
            instances are launched for you, the app opens, and its feed scrolls — one
            upward swipe on every selected device, on a repeating timer. ("Start selected"
            just opens the instances without scrolling.)
          </p>
        </div>
        <div className="page__headactions">
          <Button variant="ghost" onClick={() => void refreshDevices()} loading={scanning}>
            Rescan
          </Button>
        </div>
      </header>

      {!supported && (
        <div className="notice notice--info" style={{ marginBottom: 14 }}>
          <div>
            LDPlayer only runs on Windows, but adb devices — like an Android Studio
            emulator — still work here. They just can't be launched from this page;
            start them yourself, then Rescan.
          </div>
        </div>
      )}

      <section className="section">
        <div className="section__head">
          <h2 className="section__title">Devices</h2>
          <span className="section__hint">{selected.size} selected</span>
        </div>

        {instances.length === 0 ? (
          <div className="empty">
            <div className="empty__title">No devices found</div>
            <div className="empty__text">Start an emulator (LDPlayer or Android Studio), then Rescan.</div>
          </div>
        ) : (
          <ul className="scrolllist">
            {instances.map((d) => {
              const on = selected.has(d.id);
              return (
                <li key={d.id} className={`scrolllist__row ${on ? "scrolllist__row--on" : ""}`.trim()}>
                  <label className="scrolllist__pick">
                    <input type="checkbox" checked={on} onChange={() => toggle(d.id)} />
                    <span className="scrolllist__name">{d.name}</span>
                    <span className="scrolllist__kind">
                      {d.kind === "ldplayer" ? "LDPlayer" : "adb"}
                    </span>
                  </label>
                  <StatusBadge
                    tone={
                      d.state === "online"
                        ? "success"
                        : d.state === "booting"
                          ? "active"
                          : d.state === "unreachable"
                            ? "warning"
                            : "muted"
                    }
                    pulse={d.state === "booting"}
                  >
                    {DEVICE_STATE_LABEL[d.state]}
                  </StatusBadge>
                </li>
              );
            })}
          </ul>
        )}
      </section>

      <section className="section">
        <div className="section__head">
          <h2 className="section__title">Scroll</h2>
        </div>
        <div className="scrollctl">
          <label className="scrollctl__field">
            <span>Open</span>
            <select
              className="scrollctl__select"
              value={appId}
              disabled={scrolling}
              onChange={(e) => setAppId(e.target.value)}
            >
              {availableApps.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.label}
                </option>
              ))}
              <option value="">Current screen (don't open an app)</option>
            </select>
            {loadingApps && <span className="scrollctl__loading">checking apps…</span>}
          </label>
          <label className="scrollctl__field">
            <span>Every</span>
            <input
              type="number"
              min={1}
              max={120}
              step={1}
              value={seconds}
              disabled={scrolling}
              onChange={(e) => setSeconds(Math.max(1, Math.min(120, Number(e.target.value) || 1)))}
            />
            <span>seconds</span>
          </label>

          <div className="scrollctl__actions">
            <Button
              variant="ghost"
              onClick={() => void startInstances()}
              loading={starting}
              disabled={selected.size === 0}
            >
              Start selected
            </Button>
            {scrolling ? (
              <Button variant="danger" onClick={() => void stopScroll()}>
                Stop auto-scroll
              </Button>
            ) : (
              <Button onClick={() => void startScroll()} disabled={selected.size === 0}>
                Start auto-scroll
              </Button>
            )}
          </div>
        </div>
        {scrolling && (
          <p className="scrollctl__status">
            {(chosenApp?.label ?? "Feed")} on{" "}
            {selected.size} device{selected.size === 1 ? "" : "s"} — scrolling every {seconds}s…
          </p>
        )}
      </section>
    </div>
  );
}

function messageOf(e: unknown): string {
  if (typeof e === "object" && e && "message" in e) return String((e as { message: unknown }).message);
  if (e instanceof Error) return e.message;
  return "Something went wrong.";
}
