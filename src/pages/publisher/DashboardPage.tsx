import { useMemo } from "react";
import { Button } from "@/components/ui/Button";
import { usePublish } from "@/components/publish/PublishProvider";
import { JobList } from "@/pages/publisher/JobList";
import { StatusBadge } from "@/components/publish/StatusDot";
import { DEVICE_STATE_LABEL } from "@/lib/ldplayer";
import { isJobActive } from "@/lib/publish";
import { PlatformMark } from "@/components/publish/PlatformMark";

type Route = "pub-accounts" | "pub-publish" | "pub-settings";

/** What is connected, what is running, and what the queue is doing. */
export function PublisherDashboardPage({ onNavigate }: { onNavigate: (r: Route) => void }) {
  const { devices, accounts, jobs, environment, scanning, refreshDevices } = usePublish();

  const stats = useMemo(() => {
    const connected = accounts.filter((a) => a.status === "connected").length;
    const running = devices.filter((d) => d.state === "online").length;
    const active = jobs.filter((j) => isJobActive(j.status)).length;
    const attention = jobs.filter((j) => j.status === "needs_attention").length;
    const failed = jobs.filter((j) => j.status === "failed").length;
    return { connected, running, active, attention, failed };
  }, [accounts, devices, jobs]);

  const ready = environment?.adb_available === true;

  return (
    <div className="page">
      <header className="page__head">
        <div>
          <h1 className="page__title">Publishing</h1>
          <p className="page__lede">
            Send one video to every LDPlayer instance you have signed in, without
            dragging files into the emulator.
          </p>
        </div>
        <div className="page__headactions">
          <Button variant="ghost" onClick={() => void refreshDevices()} loading={scanning}>
            Rescan
          </Button>
          <Button onClick={() => onNavigate("pub-publish")} disabled={!ready}>
            New post
          </Button>
        </div>
      </header>

      {/* The fix depends on the OS, so the advice has to as well: telling a Mac
          user to install LDPlayer sends them after something that does not
          exist for their platform. */}
      {!ready && (
        <div className="notice notice--warn">
          <div>
            <strong>ADB wasn't found.</strong>{" "}
            {environment?.ldplayer_supported === false ? (
              <>
                LDPlayer is Windows-only, so there's nothing to install here. Install
                Android platform-tools (<code>brew install --cask android-platform-tools</code>
                ), or point this app at an <code>adb</code> executable. You can still
                publish to any emulator or phone ADB can see.
              </>
            ) : (
              <>
                Install LDPlayer — it ships its own <code>adb.exe</code> — then press
                Re-detect. If it's already installed somewhere unusual, set its folder in
                Settings.
              </>
            )}
          </div>
          <Button variant="ghost" onClick={() => onNavigate("pub-settings")}>
            Open Settings
          </Button>
        </div>
      )}

      <div className="statrow">
        <Stat label="Accounts connected" value={stats.connected} total={accounts.length} />
        <Stat label="Emulators running" value={stats.running} total={devices.length} />
        <Stat label="Jobs in flight" value={stats.active} />
        <Stat label="Waiting for you" value={stats.attention} tone={stats.attention ? "warning" : undefined} />
        <Stat label="Failed" value={stats.failed} tone={stats.failed ? "danger" : undefined} />
      </div>

      <div className="dashcols">
        <section className="section">
          <div className="section__head">
            <h2 className="section__title">Emulators</h2>
            <button className="linkbtn" type="button" onClick={() => onNavigate("pub-accounts")}>
              Manage
            </button>
          </div>
          {devices.length === 0 ? (
            <div className="empty">
              <div className="empty__title">No devices found</div>
              <div className="empty__text">Start an LDPlayer instance, then Rescan.</div>
            </div>
          ) : (
            <ul className="minilist">
              {devices.map((d) => (
                <li key={d.id} className="minilist__row">
                  <span className="minilist__name">{d.name}</span>
                  <span className="minilist__meta">
                    {accounts.filter((a) => a.ldplayer_instance_id === d.id).length} account(s)
                  </span>
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
              ))}
            </ul>
          )}
        </section>

        <section className="section">
          <div className="section__head">
            <h2 className="section__title">Accounts</h2>
            <button className="linkbtn" type="button" onClick={() => onNavigate("pub-accounts")}>
              Manage
            </button>
          </div>
          {accounts.length === 0 ? (
            <div className="empty">
              <div className="empty__title">No accounts yet</div>
              <div className="empty__text">
                Add the social apps already installed on your instances.
              </div>
            </div>
          ) : (
            <ul className="minilist">
              {accounts.map((a) => (
                <li key={a.id} className="minilist__row">
                  <PlatformMark platform={a.platform} size={20} />
                  <span className="minilist__name">{a.name}</span>
                  <StatusBadge tone={a.status === "connected" ? "success" : "muted"}>
                    {a.device_name ?? a.ldplayer_instance_id}
                  </StatusBadge>
                </li>
              ))}
            </ul>
          )}
        </section>
      </div>

      <section className="section">
        <div className="section__head">
          <h2 className="section__title">Recent activity</h2>
          <button className="linkbtn" type="button" onClick={() => onNavigate("pub-publish")}>
            Open queue
          </button>
        </div>
        <JobList jobs={jobs.slice(0, 6)} />
      </section>
    </div>
  );
}

function Stat({
  label,
  value,
  total,
  tone,
}: {
  label: string;
  value: number;
  total?: number;
  tone?: "warning" | "danger";
}) {
  return (
    <div className={`stat ${tone ? `stat--${tone}` : ""}`.trim()}>
      <div className="stat__value">
        {value}
        {total !== undefined && <span className="stat__total">/{total}</span>}
      </div>
      <div className="stat__label">{label}</div>
    </div>
  );
}
