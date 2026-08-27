import { useState } from "react";
import { Button } from "@/components/ui/Button";
import { useToast } from "@/components/ui/Toast";
import { usePublish } from "@/components/publish/PublishProvider";
import { PlatformMark, platformLabel } from "@/components/publish/PlatformMark";
import { StatusBadge } from "@/components/publish/StatusDot";
import {
  DEVICE_STATE_LABEL,
  ldplayerStart,
  ldplayerStop,
  ldplayerConnect,
  ldplayerConnectEndpoint,
  type DeviceView,
} from "@/lib/ldplayer";
import {
  ACCOUNT_STATUS_LABEL,
  publishAddAccount,
  publishDiscoverAccounts,
  publishRemoveAccount,
  publishRenameAccount,
  publishSetProfileName,
  publishAddPage,
  publishDiscoverPages,
  publishRemovePage,
  type AccountStatus,
  type AccountView,
  type DiscoveredApp,
} from "@/lib/publish";
import { AlertIcon, BoltIcon, StopIcon, TrashIcon } from "@/components/ui/icons";

const ACCOUNT_TONE: Record<AccountStatus, "success" | "warning" | "danger" | "muted"> = {
  connected: "success",
  app_missing: "warning",
  device_offline: "muted",
  device_missing: "danger",
};

const DEVICE_TONE = {
  online: "success",
  booting: "active",
  unreachable: "warning",
  stopped: "muted",
} as const;

/**
 * Emulators and the accounts on them.
 *
 * The two are on one page on purpose. An account is meaningless without its
 * emulator — "Instagram · LDPlayer #2 · Connected" is one fact, not two — and
 * splitting them would make the most common question ("why is this one grey?")
 * require two tabs to answer.
 */
export function PublisherAccountsPage() {
  const { devices, accounts, environment, scanning, refreshDevices, refreshAccounts } = usePublish();
  const toast = useToast();
  const [busy, setBusy] = useState<string | null>(null);

  // Connect failures carry the fix in the message ("turn on ADB debugging"),
  // and a toast that vanishes in four seconds is the wrong place for an
  // instruction. Keep it pinned to the device it belongs to.
  const [failure, setFailure] = useState<{ id: string; text: string } | null>(null);

  const act = async (id: string, label: string, run: () => Promise<unknown>) => {
    setBusy(id);
    setFailure((prev) => (prev?.id === id ? null : prev));
    try {
      await run();
      await refreshDevices();
      await refreshAccounts();
    } catch (e) {
      setFailure({ id, text: message(e) });
      toast("error", `${label} failed`);
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="page">
      <header className="page__head">
        <div>
          <h1 className="page__title">Accounts</h1>
          <p className="page__lede">
            Each account is a social app on an LDPlayer instance that you have already
            signed into inside the emulator. This app never asks for those passwords.
          </p>
        </div>
        <Button variant="ghost" onClick={() => void refreshDevices()} loading={scanning}>
          Rescan
        </Button>
      </header>

      {environment && !environment.adb_available && (
        <div className="notice notice--warn">
          <AlertIcon size={16} />
          <div>
            <strong>ADB was not found.</strong>{" "}
            {environment.ldplayer_supported ? (
              <>
                If LDPlayer is installed, it's somewhere this app didn't look — set{" "}
                <em>LDPlayer folder</em> in Settings to the folder containing{" "}
                <code>ldconsole.exe</code> (right-click the LDPlayer shortcut → Open file
                location), then press Re-detect.
              </>
            ) : (
              <>
                LDPlayer is Windows-only. Install Android platform-tools, or set the ADB
                path in Settings.
              </>
            )}
            {/* Naming the folders turns "not found" into something the user can
                act on: they can see at a glance that their drive isn't listed. */}
            {environment.searched.length > 0 && <SearchedPaths paths={environment.searched} />}
          </div>
        </div>
      )}

      {environment && environment.adb_available && !environment.ldplayer_available && (
        <div className="notice">
          <AlertIcon size={16} />
          <div>
            {environment.ldplayer_supported ? (
              <>
                <strong>LDPlayer was not found.</strong> Any device ADB can see still
                works, but this app can't start or stop instances until you set the
                LDPlayer folder in Settings.
              </>
            ) : (
              <>
                <strong>LDPlayer is Windows-only.</strong> On this machine you can still
                publish to any emulator or phone ADB can see.
              </>
            )}
          </div>
        </div>
      )}

      <section className="section">
        <h2 className="section__title">Emulators</h2>
        {devices.length === 0 ? (
          <div className="empty">
            <div className="empty__title">No devices found</div>
            <div className="empty__text">
              {environment?.ldplayer_available
                ? "Create an instance in LDPlayer's Multi-Instance Manager, then press Rescan."
                : "LDPlayer instances appear here automatically once LDPlayer is detected. Until then, add a device by address below."}
            </div>
          </div>
        ) : (
          <div className="devgrid">
            {devices.map((device) => (
              <DeviceCard
                key={device.id}
                device={device}
                accounts={accounts.filter((a) => a.ldplayer_instance_id === device.id)}
                busy={busy === device.id}
                failure={failure?.id === device.id ? failure.text : null}
                onStart={() => act(device.id, "Start", () => ldplayerStart(device.id))}
                onStop={() => act(device.id, "Stop", () => ldplayerStop(device.id))}
                onConnect={() => act(device.id, "Connect", () => ldplayerConnect(device.id))}
                onAdded={refreshAccounts}
              />
            ))}
          </div>
        )}
        <ManualConnect onConnected={refreshDevices} />
      </section>

      <section className="section">
        <h2 className="section__title">Connected accounts</h2>
        {accounts.length === 0 ? (
          <div className="empty">
            <div className="empty__title">No accounts yet</div>
            <div className="empty__text">
              Start an instance above, then use "Find apps" to add the social apps
              already installed on it.
            </div>
          </div>
        ) : (
          <div className="acctlist">
            {accounts.map((account) => (
              <AccountRow
                key={account.id}
                account={account}
                busy={busy === account.id}
                onRename={(name) =>
                  act(account.id, "Rename", () => publishRenameAccount(account.id, name))
                }
                onRemove={() =>
                  act(account.id, "Remove", () => publishRemoveAccount(account.id))
                }
                onSetProfileName={(profileName) =>
                  act(account.id, "Save", () =>
                    publishSetProfileName(account.id, profileName),
                  )
                }
                onAddPage={(pageName) =>
                  act(account.id, "Add Page", () => publishAddPage(account.id, pageName))
                }
                onFindPages={() =>
                  act(account.id, "Find Pages", async () => {
                    const found = await publishDiscoverPages(account.id);
                    toast(
                      "success",
                      found.length === 0
                        ? "No Pages found on this account."
                        : `Found ${found.length} ${found.length === 1 ? "Page" : "Pages"}: ${found.join(", ")}`,
                    );
                  })
                }
                onRemovePage={(pageName) =>
                  act(account.id, "Remove Page", () =>
                    publishRemovePage(account.id, pageName),
                  )
                }
              />
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

/**
 * Attach a device by address.
 *
 * LDPlayer instances are discovered automatically, so this is the escape
 * hatch, not the main path: an install this app failed to detect, another
 * vendor's emulator, or a device on another machine. Without it, a user whose
 * LDPlayer sits somewhere unusual sees an empty list and has no way forward.
 */
function ManualConnect({ onConnected }: { onConnected: () => Promise<void> }) {
  const toast = useToast();
  const [address, setAddress] = useState("");
  const [busy, setBusy] = useState(false);

  const connect = async () => {
    setBusy(true);
    try {
      const device = await ldplayerConnectEndpoint(address);
      await onConnected();
      setAddress("");
      toast("success", `Connected to ${device.name}.`);
    } catch (e) {
      toast("error", message(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      className="manualconnect"
      onSubmit={(e) => {
        e.preventDefault();
        if (address.trim()) void connect();
      }}
    >
      <div className="manualconnect__text">
        <div className="manualconnect__label">Add a device by address</div>
        <div className="manualconnect__hint">
          LDPlayer instances show up on their own. Use this for an emulator this app
          didn't find — LDPlayer's first instance is usually port 5555, the second 5557.
        </div>
      </div>
      <input
        className="input input--sm"
        value={address}
        placeholder="127.0.0.1:5555"
        spellCheck={false}
        onChange={(e) => setAddress(e.target.value)}
      />
      <Button type="submit" variant="ghost" loading={busy} disabled={!address.trim()}>
        Connect
      </Button>
    </form>
  );
}

/** The folders detection tried, collapsed by default. */
function SearchedPaths({ paths }: { paths: string[] }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="searched">
      <button className="linkbtn" type="button" onClick={() => setOpen((o) => !o)}>
        {open ? "Hide" : `Show the ${paths.length} folders that were checked`}
      </button>
      {open && (
        <ul className="searched__list">
          {paths.map((p) => (
            <li key={p}>{p}</li>
          ))}
        </ul>
      )}
    </div>
  );
}

function DeviceCard({
  device,
  accounts,
  busy,
  failure,
  onStart,
  onStop,
  onConnect,
  onAdded,
}: {
  device: DeviceView;
  accounts: AccountView[];
  busy: boolean;
  failure: string | null;
  onStart: () => void;
  onStop: () => void;
  onConnect: () => void;
  onAdded: () => Promise<void>;
}) {
  const toast = useToast();
  const [found, setFound] = useState<DiscoveredApp[] | null>(null);
  const [finding, setFinding] = useState(false);

  const isLd = device.kind === "ldplayer";
  const online = device.state === "online";

  const find = async () => {
    setFinding(true);
    try {
      const apps = await publishDiscoverAccounts(device.id);
      setFound(apps);
      if (apps.length === 0) {
        toast("info", `No supported social apps are installed on ${device.name}.`);
      }
    } catch (e) {
      toast("error", `Could not read the app list: ${message(e)}`);
    } finally {
      setFinding(false);
    }
  };

  const add = async (app: DiscoveredApp) => {
    try {
      await publishAddAccount({
        name: `${app.label} · ${device.name}`,
        platform: app.platform,
        deviceId: device.id,
        package: app.package,
      });
      await onAdded();
      toast("success", `Added ${app.label} on ${device.name}.`);
      setFound((prev) => prev?.filter((a) => a.package !== app.package) ?? null);
    } catch (e) {
      toast("error", `Could not add the account: ${message(e)}`);
    }
  };

  const alreadyAdded = (pkg: string) => accounts.some((a) => a.package_name === pkg);

  return (
    <article className="devcard">
      <header className="devcard__head">
        <div>
          <div className="devcard__name">{device.name}</div>
          <div className="devcard__meta">
            {isLd ? `LDPlayer #${device.index}` : "ADB device"}
            {device.serial ? ` · ${device.serial}` : ""}
            {device.android_release ? ` · Android ${device.android_release}` : ""}
          </div>
        </div>
        <StatusBadge tone={DEVICE_TONE[device.state]} pulse={device.state === "booting"}>
          {DEVICE_STATE_LABEL[device.state]}
        </StatusBadge>
      </header>

      {failure && <div className="devcard__error devcard__error--strong">{failure}</div>}
      {!failure && device.error && <div className="devcard__error">{device.error}</div>}

      <div className="devcard__accounts">
        {accounts.length === 0 ? (
          <span className="devcard__none">No accounts on this instance</span>
        ) : (
          accounts.map((a) => (
            <span key={a.id} className="minichip">
              <PlatformMark platform={a.platform} size={16} />
              {a.name}
            </span>
          ))
        )}
      </div>

      <footer className="devcard__actions">
        {isLd && !online && (
          <Button variant="ghost" icon={<BoltIcon size={14} />} loading={busy} onClick={onStart}>
            Start
          </Button>
        )}
        {isLd && online && (
          <Button variant="ghost" icon={<StopIcon size={14} />} loading={busy} onClick={onStop}>
            Stop
          </Button>
        )}
        {!online && (
          <Button variant="ghost" loading={busy} onClick={onConnect}>
            Connect
          </Button>
        )}
        <Button variant="ghost" disabled={!online} loading={finding} onClick={() => void find()}>
          Find apps
        </Button>
      </footer>

      {found && found.length > 0 && (
        <div className="devcard__found">
          {found.map((app) => (
            <div key={app.package} className="foundrow">
              <PlatformMark platform={app.platform} size={22} />
              <div className="foundrow__text">
                <div className="foundrow__label">
                  {app.label}
                  {/* Two "Facebook" rows look identical otherwise, and the
                      difference only shows up when a job stops. */}
                  {app.supports_auto_post ? (
                    <span className="foundrow__tag foundrow__tag--auto">
                      auto-post works
                    </span>
                  ) : (
                    <span className="foundrow__tag">manual post only</span>
                  )}
                </div>
                <code className="foundrow__pkg">{app.package}</code>
              </div>
              {alreadyAdded(app.package) ? (
                <span className="foundrow__added">Added</span>
              ) : (
                <Button variant="ghost" onClick={() => void add(app)}>
                  Add
                </Button>
              )}
            </div>
          ))}
        </div>
      )}
    </article>
  );
}

function AccountRow({
  account,
  busy,
  onRename,
  onRemove,
  onSetProfileName,
  onAddPage,
  onRemovePage,
  onFindPages,
}: {
  account: AccountView;
  busy: boolean;
  onRename: (name: string) => void;
  onRemove: () => void;
  onSetProfileName: (profileName: string) => void;
  onAddPage: (pageName: string) => void;
  onRemovePage: (pageName: string) => void;
  onFindPages: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(account.name);
  const [editingProfile, setEditingProfile] = useState(false);
  const [profileDraft, setProfileDraft] = useState(account.profile_name ?? "");
  const [pageDraft, setPageDraft] = useState("");

  return (
    <div className="acctrow">
      <PlatformMark platform={account.platform} />
      <div className="acctrow__text">
        {editing ? (
          <form
            className="acctrow__edit"
            onSubmit={(e) => {
              e.preventDefault();
              const name = draft.trim();
              if (name && name !== account.name) onRename(name);
              setEditing(false);
            }}
          >
            <input
              className="input input--sm"
              value={draft}
              autoFocus
              onChange={(e) => setDraft(e.target.value)}
              onBlur={() => setEditing(false)}
            />
          </form>
        ) : (
          <button className="acctrow__name" type="button" onClick={() => setEditing(true)}>
            {account.name}
          </button>
        )}
        <div className="acctrow__meta">
          {account.device_name ?? account.ldplayer_instance_id} · {account.package_name}
        </div>
        {account.detail && <div className="acctrow__detail">{account.detail}</div>}
        {!account.available && (
          <div className="acctrow__note">
            Publishing to {platformLabel(account.platform)} is
            switched off for now, so this account isn’t offered as a target. It stays
            here, and can be removed, until the platform is turned back on.
          </div>
        )}
        {/* Pages are the things people actually publish to, so they are listed
            on the account rather than hidden behind it. Adding one here is the
            manual route; reading them out of the app itself comes next. */}
        {account.platform === "facebook" && (
          <div className="acctrow__pages">
            <div className="acctrow__pageshead">
              <span>
                {account.pages.length === 0
                  ? "No Pages yet — read them from the app, or add one by hand"
                  : `Posts to ${account.pages.length} ${
                      account.pages.length === 1 ? "Page" : "Pages"
                    }`}
              </span>
              {/* Reads the app's own profile switcher. Needs the emulator
                  running, which is why it says so rather than failing oddly. */}
              <Button
                variant="ghost"
                disabled={busy || account.status !== "connected"}
                onClick={onFindPages}
              >
                {account.pages.length === 0 ? "Find Pages" : "Refresh"}
              </Button>
            </div>
            {account.pages.length > 0 && (
              <ul className="acctrow__pagelist">
                {account.pages.map((page) => (
                  <li key={page.name} className="acctrow__page">
                    <span className="acctrow__pagename">{page.name}</span>
                    <button
                      className="linkbtn"
                      type="button"
                      disabled={busy}
                      onClick={() => onRemovePage(page.name)}
                    >
                      Remove
                    </button>
                  </li>
                ))}
              </ul>
            )}
            <form
              className="acctrow__pageadd"
              onSubmit={(e) => {
                e.preventDefault();
                const name = pageDraft.trim();
                if (!name) return;
                onAddPage(name);
                setPageDraft("");
              }}
            >
              <input
                className="input input--sm"
                value={pageDraft}
                placeholder="Page name, exactly as Facebook shows it"
                onChange={(e) => setPageDraft(e.target.value)}
              />
              <Button variant="ghost" type="submit" disabled={busy || !pageDraft.trim()}>
                Add Page
              </Button>
            </form>
          </div>
        )}
        {/* Identity check. Empty means off, which is why the prompt says what
            turning it on buys rather than just naming the field. */}
        {editingProfile ? (
          <form
            className="acctrow__edit"
            onSubmit={(e) => {
              e.preventDefault();
              onSetProfileName(profileDraft.trim());
              setEditingProfile(false);
            }}
          >
            <input
              className="input input--sm"
              value={profileDraft}
              autoFocus
              placeholder="Name exactly as the app shows it on the composer"
              onChange={(e) => setProfileDraft(e.target.value)}
              onBlur={() => setEditingProfile(false)}
            />
          </form>
        ) : (
          <button
            className="acctrow__note acctrow__note--action"
            type="button"
            onClick={() => setEditingProfile(true)}
          >
            {account.profile_name
              ? `Posts as ${account.profile_name} — checked before anything is submitted`
              : "Set who this posts as, and every post is checked against it before it goes out"}
          </button>
        )}
        {/* Said here, before jobs are queued, rather than after one stops. */}
        {!account.supports_auto_post && (
          <div className="acctrow__note">
            This app draws its own screen, so “Tap Post automatically” can’t work on it —
            publishing works, but the final tap stays manual. The full (non-Lite) app
            supports automation.
          </div>
        )}
      </div>
      <StatusBadge tone={ACCOUNT_TONE[account.status]}>
        {ACCOUNT_STATUS_LABEL[account.status]}
      </StatusBadge>
      <Button
        variant="ghost"
        icon={<TrashIcon size={14} />}
        loading={busy}
        onClick={onRemove}
        aria-label={`Remove ${account.name}`}
        title="Remove this account (the app on the emulator is left alone)"
      />
    </div>
  );
}

/** Rust errors arrive as `{ code, message }`; anything else is a bug in us. */
function message(e: unknown): string {
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}
