import { useEffect, useMemo, useState } from "react";
import { Button } from "@/components/ui/Button";
import { usePublish } from "@/components/publish/PublishProvider";
import { StatusBadge } from "@/components/publish/StatusDot";
import { useToast } from "@/components/ui/Toast";
import {
  DEVICE_STATE_LABEL,
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
  const [scrolling, setScrolling] = useState(false);
  const [starting, setStarting] = useState(false);

  // Only LDPlayer instances scroll here (plain adb devices aren't launchable).
  const instances = useMemo(
    () => devices.filter((d) => d.kind === "ldplayer"),
    [devices],
  );

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

  const toggle = (id: string) =>
    setSelected((prev) => {
      const n = new Set(prev);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });

  const selectedIds = [...selected];
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
    try {
      await ldplayerAutoscrollStart(selectedIds, Math.round(seconds * 1000));
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
            Pick the LDPlayer instances, start them, then auto-scroll their feeds —
            one upward swipe on every selected instance, on a repeating timer.
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
          <div>LDPlayer only runs on Windows, so this page has nothing to control here.</div>
        </div>
      )}

      <section className="section">
        <div className="section__head">
          <h2 className="section__title">Instances</h2>
          <span className="section__hint">{selected.size} selected</span>
        </div>

        {instances.length === 0 ? (
          <div className="empty">
            <div className="empty__title">No LDPlayer instances found</div>
            <div className="empty__text">Create an instance in LDPlayer, then Rescan.</div>
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
            Scrolling {selected.size} instance{selected.size === 1 ? "" : "s"} every {seconds}s…
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
