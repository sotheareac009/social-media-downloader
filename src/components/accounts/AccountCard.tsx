import { useState } from "react";
import {
  formatConnectedSince,
  type AccountView,
  type ProviderDescriptor,
} from "@/lib/auth";
import { AlertIcon, CheckIcon } from "@/components/ui/icons";
import { BRAND_COLOR, ProviderLogo } from "./ProviderLogo";
import { ConnectButton } from "./ConnectButton";

interface Props {
  descriptor: ProviderDescriptor;
  account: AccountView;
  busy: boolean;
  blocked: boolean;
  /** Persistent setup guidance from the platform's own refusal, if any. */
  notice?: string | null;
  onConnect: () => void;
  onDisconnect: () => void;
}

export function AccountCard({
  descriptor,
  account,
  busy,
  blocked,
  notice,
  onConnect,
  onDisconnect,
}: Props) {
  const connected = account.connected;

  return (
    <article
      className={`card ${connected ? "card--connected" : ""}`.trim()}
      style={{ ["--brand" as string]: BRAND_COLOR[descriptor.id] }}
    >
      <span className="card__edge" />

      <div className="card__body">
        <ProviderLogo provider={descriptor.id} />

        <div className="card__text">
          <h2 className="card__name">
            {descriptor.display_name}
            <StatusBadge account={account} descriptor={descriptor} busy={busy} />
          </h2>
          <p className="card__meta">
            {subtitle(descriptor, account, busy)}
          </p>
        </div>

        <div className="card__actions">
          <ConnectButton
            descriptor={descriptor}
            account={account}
            busy={busy}
            blocked={blocked}
            onConnect={onConnect}
            onDisconnect={onDisconnect}
          />
        </div>
      </div>

      {/* Height-animated so connecting expands the card rather than snapping. */}
      <div className="card__identity">
        <div>
          {connected && <Identity account={account} />}
        </div>
      </div>

      {!connected && !descriptor.configured && (
        <SetupNotice provider={descriptor.id} />
      )}

      {/* A platform refusal we can act on. Deliberately not a toast: this is
          multi-step instructions the user needs to keep on screen. */}
      {notice && (
        <div className="notice notice--danger">
          <span className="notice__icon">
            <AlertIcon size={14} />
          </span>
          <div>{notice}</div>
        </div>
      )}
    </article>
  );
}

function StatusBadge({
  account,
  descriptor,
  busy,
}: {
  account: AccountView;
  descriptor: ProviderDescriptor;
  busy: boolean;
}) {
  if (busy && !account.connected) {
    return (
      <span className="badge badge--muted">
        Authorizing<span className="badge__dot" />
      </span>
    );
  }
  if (account.needs_reauth) {
    return (
      <span className="badge badge--warning">
        <AlertIcon size={11} />
        Reconnect needed
      </span>
    );
  }
  if (account.connected) {
    return (
      <span className="badge badge--success">
        <CheckIcon size={11} />
        Connected
      </span>
    );
  }
  if (!descriptor.configured) {
    return <span className="badge badge--muted">Needs setup</span>;
  }
  return <span className="badge badge--muted">Not connected</span>;
}

function subtitle(
  descriptor: ProviderDescriptor,
  account: AccountView,
  busy: boolean,
): string {
  if (busy && !account.connected) return "Finish signing in in your browser…";
  if (account.connected) return formatConnectedSince(account.created_at);
  if (!descriptor.configured) return "No client ID is configured for this build";
  return `Sign in with ${descriptor.display_name} to link your account`;
}

function Identity({ account }: { account: AccountView }) {
  const [avatarFailed, setAvatarFailed] = useState(false);
  const name = account.display_name ?? "Connected account";
  const showAvatar = account.avatar_url && !avatarFailed;

  return (
    <div className="identity">
      {showAvatar ? (
        <img
          className="identity__avatar"
          src={account.avatar_url!}
          alt=""
          referrerPolicy="no-referrer"
          onError={() => setAvatarFailed(true)}
        />
      ) : (
        <div className="identity__fallback" aria-hidden>
          {initial(name)}
        </div>
      )}

      <div className="identity__text">
        <div className="identity__name">{name}</div>
        <div className="identity__sub">
          {account.email ?? `ID ${truncateId(account.external_id)}`}
        </div>
      </div>
    </div>
  );
}

function initial(name: string): string {
  return name.trim().charAt(0).toUpperCase() || "?";
}

/** Provider user IDs are long; show enough to recognise, not enough to clutter. */
function truncateId(id: string | null): string {
  if (!id) return "unknown";
  return id.length > 12 ? `${id.slice(0, 6)}…${id.slice(-4)}` : id;
}

/** The env keys each provider needs before its Connect button is enabled. */
const SETUP_VARS: Record<string, string[]> = {
  google: ["GOOGLE_CLIENT_ID"],
  facebook: [
    "FACEBOOK_CLIENT_ID",
    "FACEBOOK_CLIENT_SECRET",
    "FACEBOOK_REDIRECT_URI",
  ],
  instagram: [
    "INSTAGRAM_CLIENT_ID",
    "INSTAGRAM_CLIENT_SECRET",
    "INSTAGRAM_REDIRECT_URI",
  ],
  tiktok: ["TIKTOK_CLIENT_KEY", "TIKTOK_CLIENT_SECRET"],
};

function SetupNotice({ provider }: { provider: string }) {
  const vars = SETUP_VARS[provider] ?? [];

  return (
    <div className="notice">
      <span className="notice__icon">
        <AlertIcon size={14} />
      </span>
      <div>
        Set{" "}
        {vars.map((v, i) => (
          <span key={v}>
            {i > 0 && (i === vars.length - 1 ? " and " : ", ")}
            <code>{v}</code>
          </span>
        ))}{" "}
        before launching the app to enable this provider.
      </div>
    </div>
  );
}
