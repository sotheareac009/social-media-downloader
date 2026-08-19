import { useCallback, useEffect, useRef, useState } from "react";
import {
  authConnect,
  authDisconnect,
  authGetAccounts,
  authGetProviders,
  friendlyMessage,
  subscribeToAuthEvents,
  toAuthError,
  type AccountView,
  type ProviderDescriptor,
  type ProviderId,
} from "@/lib/auth";
import { AccountCard } from "@/components/accounts/AccountCard";
import {
  facebookConnect,
  facebookDisconnect,
  facebookStatus,
  instagramConnect,
  instagramDisconnect,
  instagramStatus,
  type SessionStatus,
} from "@/lib/download";
import { useToast } from "@/components/ui/Toast";
import { AlertIcon, CheckIcon, ChevronRightIcon, ShieldIcon, TelegramIcon, UsersIcon } from "@/components/ui/icons";
import { telegramStatus as fetchTelegramStatus, type TelegramStatus } from "@/lib/telegram";

export function AccountsPage({
  onNavigate,
}: {
  onNavigate?: (r: "telegram" | "facebook") => void;
}) {
  const toast = useToast();
  const [providers, setProviders] = useState<ProviderDescriptor[] | null>(null);
  const [accounts, setAccounts] = useState<Record<string, AccountView>>({});
  const [busy, setBusy] = useState<ProviderId | null>(null);
  /// Setup guidance from a platform refusal, kept until the next attempt.
  const [configErrors, setConfigErrors] = useState<
    Partial<Record<ProviderId, string>>
  >({});
  const [loadError, setLoadError] = useState<string | null>(null);
  // Downloading from Instagram uses a separate credential from this page's
  // account sign-in; shown here so the two states stop looking contradictory.
  const [igDownload, setIgDownload] = useState<SessionStatus | null>(null);
  const [igBusy, setIgBusy] = useState(false);
  const [fbDownload, setFbDownload] = useState<SessionStatus | null>(null);
  const [fbBusy, setFbBusy] = useState(false);
  const [telegram, setTelegram] = useState<TelegramStatus | null>(null);

  // Guards a state update landing after unmount when a flow is abandoned.
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const applyAccount = useCallback((view: AccountView) => {
    setAccounts((prev) => ({ ...prev, [view.provider]: view }));
  }, []);

  const refresh = useCallback(async () => {
    const list = await authGetAccounts();
    if (!mounted.current) return;
    setAccounts(Object.fromEntries(list.map((a) => [a.provider, a])));
  }, []);

  useEffect(() => {
    (async () => {
      try {
        const [descriptors] = await Promise.all([authGetProviders(), refresh()]);
        if (!mounted.current) return;
        setProviders(descriptors);
        instagramStatus()
          .then((st) => mounted.current && setIgDownload(st))
          .catch(() => {});
        facebookStatus()
          .then((st) => mounted.current && setFbDownload(st))
          .catch(() => {});
        fetchTelegramStatus()
          .then((st) => mounted.current && setTelegram(st))
          .catch(() => {});
      } catch (e) {
        if (!mounted.current) return;
        setLoadError(friendlyMessage(toAuthError(e)));
      }
    })();
  }, [refresh]);

  // The backend is the source of truth for auth state. Events keep the UI
  // correct even when a flow completes outside the promise we're awaiting.
  useEffect(() => {
    const pending = subscribeToAuthEvents({
      onSuccess: (view) => applyAccount(view),
      onDisconnected: ({ revoked_remotely }) => {
        if (!revoked_remotely) {
          toast(
            "info",
            "Disconnected locally. You may also want to remove this app from the provider's account settings.",
          );
        }
        void refresh();
      },
    });
    return () => {
      void pending.then((unlisten) => unlisten());
    };
  }, [applyAccount, refresh, toast]);

  const connect = useCallback(
    async (provider: ProviderId, name: string) => {
      setBusy(provider);
      // Clear stale guidance so a fixed problem does not linger on the card.
      setConfigErrors((prev) => ({ ...prev, [provider]: undefined }));
      try {
        const view = await authConnect(provider);
        applyAccount(view);
        toast("success", `${view.display_name ?? name} is now connected.`);
      } catch (e) {
        const err = toAuthError(e);
        if (err.code === "provider_configuration") {
          // Multi-step instructions belong on the card, not in a toast that
          // vanishes before they can be read.
          setConfigErrors((prev) => ({ ...prev, [provider]: err.message }));
          toast("error", `${name} needs setup — see the card for details.`);
        } else {
          // A cancelled sign-in is a normal outcome, not a failure to shout about.
          toast(err.code === "cancelled" ? "info" : "error", friendlyMessage(err));
        }
      } finally {
        if (mounted.current) setBusy(null);
      }
    },
    [applyAccount, toast],
  );

  const disconnect = useCallback(
    async (provider: ProviderId, name: string) => {
      setBusy(provider);
      try {
        const view = await authDisconnect(provider);
        applyAccount(view);
        toast("success", `${name} disconnected.`);
      } catch (e) {
        toast("error", friendlyMessage(toAuthError(e)));
      } finally {
        if (mounted.current) setBusy(null);
      }
    },
    [applyAccount, toast],
  );

  /**
   * The same session sign-in the Downloads page runs — one flow, one place it
   * is implemented, reachable from either page.
   */
  const signInForDownloads = useCallback(async () => {
    setIgBusy(true);
    try {
      setIgDownload(await instagramConnect());
      toast("success", "Instagram connected. Reels can now be downloaded.");
    } catch (e) {
      const err = toAuthError(e);
      // Closing the window is a decision, not a failure.
      if (err.code !== "cancelled") toast("error", friendlyMessage(err));
    } finally {
      if (mounted.current) setIgBusy(false);
    }
  }, [toast]);

  const signInFacebook = useCallback(async () => {
    setFbBusy(true);
    try {
      setFbDownload(await facebookConnect());
      toast("success", "Facebook connected. Your account's videos can now be downloaded.");
    } catch (e) {
      const err = toAuthError(e);
      if (err.code !== "cancelled") toast("error", friendlyMessage(err));
    } finally {
      if (mounted.current) setFbBusy(false);
    }
  }, [toast]);

  const disconnectInstagram = useCallback(async () => {
    setIgBusy(true);
    try {
      setIgDownload(await instagramDisconnect());
      toast("info", "Instagram disconnected.");
    } catch (e) {
      toast("error", toAuthError(e).message);
    } finally {
      if (mounted.current) setIgBusy(false);
    }
  }, [toast]);

  const disconnectFacebook = useCallback(async () => {
    setFbBusy(true);
    try {
      setFbDownload(await facebookDisconnect());
      toast("info", "Facebook disconnected.");
    } catch (e) {
      toast("error", toAuthError(e).message);
    } finally {
      if (mounted.current) setFbBusy(false);
    }
  }, [toast]);

  const connectedCount = Object.values(accounts).filter((a) => a.connected).length;

  return (
    <div className="page">
      <header className="page__header rise">
        <span className="page__eyebrow">
          <UsersIcon size={12} />
          Accounts
        </span>
        <h1 className="page__title">Connected accounts</h1>
        <p className="page__lede">
          Link a platform to sign in with your own account. You'll authorize in
          your browser, on the platform's own page — this app never sees your
          password.
        </p>
      </header>

      {loadError && (
        <div className="notice notice--danger" style={{ margin: "0 0 16px" }}>
          <span className="notice__icon">
            <AlertIcon size={14} />
          </span>
          <div>{loadError}</div>
        </div>
      )}

      <div className="stack">
        {providers === null
          ? [0, 1].map((i) => <CardSkeleton key={i} />)
          : providers.map((descriptor, i) => {
              const account =
                accounts[descriptor.id] ?? blankAccount(descriptor.id);
              return (
                <div
                  key={descriptor.id}
                  className="rise"
                  style={{ animationDelay: `${i * 60}ms` }}
                >
                  <AccountCard
                    descriptor={descriptor}
                    account={account}
                    downloadConnected={
                      (descriptor.id === "instagram" && igDownload?.connected === true) ||
                      (descriptor.id === "facebook" && fbDownload?.connected === true)
                    }
                    onDownloadSignIn={
                      descriptor.id === "instagram" && !descriptor.configured
                        ? () => void signInForDownloads()
                        : descriptor.id === "facebook"
                          ? () => void signInFacebook()
                          : undefined
                    }
                    downloadBusy={descriptor.id === "facebook" ? fbBusy : igBusy}
                    downloadName={
                      descriptor.id === "instagram"
                        ? igDownload?.display_name ?? null
                        : descriptor.id === "facebook"
                          ? fbDownload?.display_name ?? null
                          : null
                    }
                    downloadAvatar={
                      descriptor.id === "instagram"
                        ? igDownload?.avatar_url ?? null
                        : descriptor.id === "facebook"
                          ? fbDownload?.avatar_url ?? null
                          : null
                    }
                    onDownloadDisconnect={
                      descriptor.id === "instagram" && igDownload?.connected
                        ? () => void disconnectInstagram()
                        : descriptor.id === "facebook" && fbDownload?.connected
                          ? () => void disconnectFacebook()
                          : undefined
                    }
                    downloadNote={
                      descriptor.id === "instagram" && igDownload?.connected
                        ? "Reels and posts will download using this session. Connecting an account below is optional — it only adds your name and avatar, and is not needed for downloading."
                        : descriptor.id === "facebook" && fbDownload?.connected
                          ? "Your Facebook videos will download using this session — including ones that need a login. Signing in for an account below is separate and optional."
                          : null
                    }
                    busy={busy === descriptor.id}
                    blocked={busy !== null && busy !== descriptor.id}
                    notice={configErrors[descriptor.id]}
                    onConnect={() =>
                      void connect(descriptor.id, descriptor.display_name)
                    }
                    onDisconnect={() =>
                      void disconnect(descriptor.id, descriptor.display_name)
                    }
                    onOpenDetail={
                      descriptor.id === "facebook"
                        ? () => onNavigate?.("facebook")
                        : undefined
                    }
                  />
                </div>
              );
            })}
      </div>

      <TelegramAccountCard
        status={telegram}
        onOpen={() => onNavigate?.("telegram")}
      />

      <Assurance connectedCount={connectedCount} />
    </div>
  );
}

function blankAccount(provider: ProviderId): AccountView {
  return {
    provider,
    connected: false,
    external_id: null,
    display_name: null,
    avatar_url: null,
    email: null,
    created_at: null,
    last_used_at: null,
    needs_reauth: false,
  };
}

function CardSkeleton() {
  return (
    <div className="card">
      <div className="card__body">
        <div className="skeleton" style={{ width: 42, height: 42, borderRadius: 13 }} />
        <div className="card__text">
          <div className="skeleton" style={{ width: 128, height: 14 }} />
          <div
            className="skeleton"
            style={{ width: 208, height: 11, marginTop: 8 }}
          />
        </div>
        <div className="skeleton" style={{ width: 104, height: 34, borderRadius: 8 }} />
      </div>
    </div>
  );
}

/** Name the OS store the Rust side is actually using, so the claim is exact. */
function secureStoreName(): string {
  const ua = navigator.userAgent;
  if (ua.includes("Mac")) return "your macOS Keychain";
  if (ua.includes("Windows")) return "Windows Credential Manager";
  return "your system's Secret Service keyring";
}

/**
 * Telegram on the Accounts list, alongside the OAuth providers. Telegram's
 * actual sign-in is a multi-step phone flow that lives on its own page, so
 * this card reflects status and links there rather than logging in inline.
 */
function TelegramAccountCard({
  status,
  onOpen,
}: {
  status: TelegramStatus | null;
  onOpen: () => void;
}) {
  const connected = status?.connected === true;
  return (
    <article
      className={`card ${connected ? "card--connected" : ""} ${connected ? "card--clickable" : ""}`.trim()}
      style={{ ["--brand" as string]: "#229ED9", marginTop: 12 }}
      onClick={connected ? onOpen : undefined}
      role={connected ? "button" : undefined}
      tabIndex={connected ? 0 : undefined}
      onKeyDown={
        connected
          ? (e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                onOpen();
              }
            }
          : undefined
      }
      title={connected ? "View Telegram details" : undefined}
    >
      <span className="card__edge" />
      <div className="card__body">
        <div className="logo logo--telegram" aria-hidden>
          <TelegramIcon size={21} className="" />
        </div>
        <div className="card__text">
          <h2 className="card__name">
            Telegram
            {connected ? (
              <span className="badge badge--success">
                <CheckIcon size={11} /> Connected
              </span>
            ) : (
              <span className="badge badge--muted">Not connected</span>
            )}
          </h2>
          <p className="card__meta">
            {connected
              ? status?.display_name ?? "Signed in"
              : "Sign in with your phone number"}
          </p>
        </div>
        <div className="card__actions" onClick={(e) => e.stopPropagation()}>
          {connected && (
            <span className="card__detailhint">
              Details <ChevronRightIcon size={14} />
            </span>
          )}
          <button className="btn btn--ghost" type="button" onClick={onOpen}>
            {connected ? "Manage" : "Connect"}
          </button>
        </div>
      </div>
    </article>
  );
}

function Assurance({ connectedCount }: { connectedCount: number }) {
  return (
    <section className="assurance rise" style={{ animationDelay: "160ms" }}>
      <div className="assurance__title">
        <ShieldIcon size={14} />
        How your credentials are handled
      </div>
      <ul className="assurance__list">
        {[
          "Sign-in happens on the platform's own page, in your system browser.",
          `Access tokens are stored in ${secureStoreName()} — never in a file, a database, or this window.`,
          "Only your name, avatar and account ID are kept locally.",
          "Disconnecting revokes the token with the provider where that's supported.",
        ].map((line) => (
          <li key={line}>
            <CheckIcon size={13} className="assurance__tick" />
            <span>{line}</span>
          </li>
        ))}
      </ul>
      {connectedCount > 0 && (
        <p
          style={{
            marginTop: 12,
            fontSize: 12,
            color: "var(--text-tertiary)",
          }}
        >
          {connectedCount} account{connectedCount === 1 ? "" : "s"} connected.
        </p>
      )}
    </section>
  );
}
