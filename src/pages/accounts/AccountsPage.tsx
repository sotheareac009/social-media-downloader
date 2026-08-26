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
import { CookiePanel } from "@/components/accounts/CookiePanel";
import type { CookiePlatform } from "@/lib/download";

/**
 * Providers whose downloads can use a pasted cookie jar.
 *
 * Google is absent because YouTube downloads need no session at all, and
 * Telegram because it is not a cookie login — it has its own card and its own
 * MTProto sign-in.
 */
const COOKIE_PLATFORMS = ["instagram", "facebook", "tiktok", "x"];
import {
  facebookConnect,
  xConnect,
  xDisconnect,
  xStatus,
  facebookDisconnect,
  facebookStatus,
  instagramConnect,
  instagramDisconnect,
  instagramStatus,
  type SessionStatus,
} from "@/lib/download";
import { useToast } from "@/components/ui/Toast";
import { AlertIcon, CheckIcon, ChevronRightIcon, ShieldIcon, TelegramIcon, UsersIcon, XIcon } from "@/components/ui/icons";
import { youtubeAccountsList, youtubeAccountRemove, type YoutubeAccount } from "@/lib/youtube";
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
  const [xDownload, setXDownload] = useState<SessionStatus | null>(null);
  const [xBusy, setXBusy] = useState(false);
  const [telegram, setTelegram] = useState<TelegramStatus | null>(null);
  const [ytAccounts, setYtAccounts] = useState<YoutubeAccount[]>([]);

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
        xStatus()
          .then((st) => mounted.current && setXDownload(st))
          .catch(() => {});
        fetchTelegramStatus()
          .then((st) => mounted.current && setTelegram(st))
          .catch(() => {});
        youtubeAccountsList()
          .then((l) => mounted.current && setYtAccounts(l))
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

  const signInX = useCallback(async () => {
    setXBusy(true);
    try {
      setXDownload(await xConnect());
      toast("success", "X connected. Your X videos can now be downloaded.");
    } catch (e) {
      const err = toAuthError(e);
      if (err.code !== "cancelled") toast("error", friendlyMessage(err));
    } finally {
      if (mounted.current) setXBusy(false);
    }
  }, [toast]);

  const disconnectX = useCallback(async () => {
    setXBusy(true);
    try {
      setXDownload(await xDisconnect());
      toast("info", "X disconnected.");
    } catch (e) {
      toast("error", toAuthError(e).message);
    } finally {
      if (mounted.current) setXBusy(false);
    }
  }, [toast]);

  const removeYtUploader = useCallback(
    async (id: string) => {
      try {
        await youtubeAccountRemove(id);
        const list = await youtubeAccountsList();
        if (mounted.current) setYtAccounts(list);
      } catch (e) {
        toast("error", toAuthError(e).message);
      }
    },
    [toast],
  );

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
                    footer={
                      descriptor.id === "x" ? (
                        <div className="xdl">
                          <div className="xdl__head">Download login</div>
                          {xDownload?.connected ? (
                            <div className="xdl__row">
                              <span className="xdl__ok">
                                <CheckIcon size={12} /> Signed in — X videos can download
                              </span>
                              <button
                                className="btn btn--ghost btn--sm"
                                type="button"
                                onClick={() => void disconnectX()}
                                disabled={xBusy}
                              >
                                Disconnect
                              </button>
                            </div>
                          ) : (
                            <div className="xdl__row">
                              <span className="xdl__hint">
                                Sign in to download videos, sensitive posts and whole profiles from X.
                                (Separate from the connect above, which is for posting.)
                              </span>
                              <button
                                className="btn btn--primary btn--sm"
                                type="button"
                                onClick={() => void signInX()}
                                disabled={xBusy}
                              >
                                {xBusy ? "Waiting…" : "Sign in to X"}
                              </button>
                            </div>
                          )}
                          <CookiePanel
                            platform="x"
                            label={descriptor.display_name}
                          />
                        </div>
                      ) : descriptor.id === "google" && ytAccounts.length > 0 ? (
                        <div className="yt-uploaders">
                          <div className="yt-uploaders__head">
                            YouTube upload accounts ({ytAccounts.length})
                          </div>
                          {ytAccounts.map((a) => (
                            <div key={a.id} className="yt-uploaders__row">
                              {a.channel_avatar || a.avatar_url ? (
                                <img
                                  src={(a.channel_avatar || a.avatar_url)!}
                                  alt=""
                                  referrerPolicy="no-referrer"
                                />
                              ) : (
                                <span className="yt-uploaders__ph">▶</span>
                              )}
                              <div className="yt-uploaders__meta">
                                <span className="yt-uploaders__name">
                                  {a.channel_title ?? a.display_name}
                                </span>
                                {a.email && (
                                  <span className="yt-uploaders__sub">{a.email}</span>
                                )}
                              </div>
                              <button
                                className="yt-uploaders__x"
                                type="button"
                                onClick={() => void removeYtUploader(a.id)}
                                aria-label="Remove account"
                                title="Remove account"
                              >
                                <XIcon size={14} />
                              </button>
                            </div>
                          ))}
                          <div className="yt-uploaders__hint">
                            Add more from the Upload page.
                          </div>
                        </div>
                      ) : COOKIE_PLATFORMS.includes(descriptor.id) ? (
                        <CookiePanel
                          platform={descriptor.id as CookiePlatform}
                          label={descriptor.display_name}
                          // Instagram and Facebook are tracked by this page
                          // already, so their cards stay in step; the rest let
                          // the panel read its own status.
                          connected={
                            descriptor.id === "instagram"
                              ? igDownload?.connected === true
                              : descriptor.id === "facebook"
                                ? fbDownload?.connected === true
                                : undefined
                          }
                          onSaved={
                            descriptor.id === "instagram"
                              ? setIgDownload
                              : descriptor.id === "facebook"
                                ? setFbDownload
                                : undefined
                          }
                        />
                      ) : undefined
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
