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
  /**
   * State of a *download* session for this provider, which is a different
   * credential from the account sign-in this card manages. Instagram is the
   * only provider that has one, and without saying so the two read as the
   * same thing — "I signed into Instagram, why does it say Needs setup?"
   */
  downloadNote?: string | null;
  /**
   * True when a download session exists for this provider. It is not an
   * account connection, but it *is* a working sign-in, so the card must not
   * keep reporting "Needs setup" as though nothing had happened.
   */
  downloadConnected?: boolean;
  /** Starts the download-session sign-in, when this provider supports one. */
  onDownloadSignIn?: () => void;
  downloadBusy?: boolean;
  onConnect: () => void;
  onDisconnect: () => void;
}

export function AccountCard({
  descriptor,
  account,
  busy,
  blocked,
  notice,
  downloadNote,
  downloadConnected = false,
  onDownloadSignIn,
  downloadBusy = false,
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
            <StatusBadge
              account={account}
              descriptor={descriptor}
              busy={busy}
              downloadConnected={downloadConnected}
            />
          </h2>
          <p className="card__meta">
            {subtitle(
              descriptor,
              account,
              busy,
              downloadConnected,
              onDownloadSignIn !== undefined,
            )}
          </p>
        </div>

        <div className="card__actions">
          <ConnectButton
            descriptor={descriptor}
            account={account}
            busy={busy}
            blocked={blocked}
            downloadConnected={downloadConnected}
            onDownloadSignIn={onDownloadSignIn}
            downloadBusy={downloadBusy}
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

      {downloadNote && (
        <div className="notice notice--info">
          <span className="notice__icon">
            <CheckIcon size={14} />
          </span>
          <div>{downloadNote}</div>
        </div>
      )}

      {!connected && !descriptor.configured && !downloadConnected && !onDownloadSignIn && (
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
  downloadConnected,
}: {
  account: AccountView;
  descriptor: ProviderDescriptor;
  busy: boolean;
  downloadConnected: boolean;
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
  // Checked before the setup states: a working download session is a real
  // sign-in, and reporting "Needs setup" next to it reads as a contradiction.
  if (downloadConnected) {
    return (
      <span className="badge badge--success">
        <CheckIcon size={11} />
        Signed in for downloads
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
  downloadConnected: boolean,
  canDownloadSignIn: boolean,
): string {
  if (busy && !account.connected) return "Finish signing in in your browser…";
  if (account.connected) return formatConnectedSince(account.created_at);
  if (downloadConnected) return "Downloads are working — nothing else needed here";
  // A missing client ID is irrelevant when the useful action is available.
  if (canDownloadSignIn) {
    return `${descriptor.display_name} blocks anonymous downloads — sign in to fetch reels and posts`;
  }
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
