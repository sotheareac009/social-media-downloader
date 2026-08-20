/**
 * Typed bridge to the Rust auth layer.
 *
 * Note what is absent: there is no way to obtain an access token from here.
 * The backend intentionally exposes only non-sensitive account metadata, so
 * this module cannot leak a credential even by accident.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type ProviderId = "google" | "facebook" | "instagram" | "tiktok" | "telegram" | "x";

export interface ProviderDescriptor {
  id: ProviderId;
  display_name: string;
  /** False when this build has no client id for the provider. */
  configured: boolean;
  supports_revocation: boolean;
  scopes: string[];
}

export interface AccountView {
  provider: ProviderId;
  connected: boolean;
  external_id: string | null;
  display_name: string | null;
  avatar_url: string | null;
  email: string | null;
  /** Unix seconds. */
  created_at: number | null;
  last_used_at: number | null;
  needs_reauth: boolean;
}

/** Structured error shape produced by `AppError`'s Serialize impl. */
export interface AuthError {
  code: string;
  message: string;
}

export function isAuthError(e: unknown): e is AuthError {
  return (
    typeof e === "object" &&
    e !== null &&
    typeof (e as AuthError).code === "string" &&
    typeof (e as AuthError).message === "string"
  );
}

export function toAuthError(e: unknown): AuthError {
  if (isAuthError(e)) return e;
  return { code: "unknown", message: "Something went wrong." };
}

/** Copy tuned for a person, not a log file. */
export function friendlyMessage(e: AuthError): string {
  switch (e.code) {
    case "cancelled":
      return "You cancelled the sign-in.";
    case "timed_out":
      return "The sign-in timed out. Try again when you're ready.";
    case "state_mismatch":
      return "The response from the provider could not be verified, so it was rejected.";
    case "flow_already_running":
      return "Another sign-in is already in progress.";
    case "provider_not_configured":
      return "This provider hasn't been configured in this build yet.";
    case "network":
      return "Couldn't reach the provider. Check your connection.";
    case "browser_launch":
      return "Couldn't open your browser.";
    case "keychain":
      return "Your system keychain is unavailable, so the account wasn't saved.";
    default:
      return e.message;
  }
}

// ---------------------------------------------------------------- commands

export const authGetProviders = () =>
  invoke<ProviderDescriptor[]>("auth_get_providers");

export const authGetAccounts = () => invoke<AccountView[]>("auth_get_accounts");

export const authGetAccount = (provider: ProviderId) =>
  invoke<AccountView>("auth_get_account", { provider });

/** Resolves once the browser round-trip completes. Can take a while. */
export const authConnect = (provider: ProviderId) =>
  invoke<AccountView>("auth_connect", { provider });

export const authDisconnect = (provider: ProviderId) =>
  invoke<AccountView>("auth_disconnect", { provider });

// ------------------------------------------------------------------ events

export interface AuthStartedEvent {
  provider: ProviderId;
}
export interface AuthFailedEvent {
  provider: ProviderId;
  code: string;
  message: string;
}
export interface AuthDisconnectedEvent {
  provider: ProviderId;
  revoked_remotely: boolean;
}

export interface AuthEventHandlers {
  onStarted?: (e: AuthStartedEvent) => void;
  onSuccess?: (e: AccountView) => void;
  onFailed?: (e: AuthFailedEvent) => void;
  onDisconnected?: (e: AuthDisconnectedEvent) => void;
}

/**
 * Subscribe to the `auth://*` event stream.
 *
 * Returns a promise for the unsubscribe function. Listeners are registered
 * asynchronously, so callers must await (or store) the result before unmount.
 */
export async function subscribeToAuthEvents(
  handlers: AuthEventHandlers,
): Promise<UnlistenFn> {
  const unlisteners: UnlistenFn[] = [];

  if (handlers.onStarted)
    unlisteners.push(
      await listen<AuthStartedEvent>("auth://started", (e) =>
        handlers.onStarted!(e.payload),
      ),
    );
  if (handlers.onSuccess)
    unlisteners.push(
      await listen<AccountView>("auth://success", (e) =>
        handlers.onSuccess!(e.payload),
      ),
    );
  if (handlers.onFailed)
    unlisteners.push(
      await listen<AuthFailedEvent>("auth://failed", (e) =>
        handlers.onFailed!(e.payload),
      ),
    );
  if (handlers.onDisconnected)
    unlisteners.push(
      await listen<AuthDisconnectedEvent>("auth://disconnected", (e) =>
        handlers.onDisconnected!(e.payload),
      ),
    );

  return () => unlisteners.forEach((u) => u());
}

// ------------------------------------------------------------------ format

export function formatConnectedSince(unixSeconds: number | null): string {
  if (!unixSeconds) return "";
  const then = new Date(unixSeconds * 1000);
  const days = Math.floor((Date.now() - then.getTime()) / 86_400_000);
  if (days <= 0) return "Connected today";
  if (days === 1) return "Connected yesterday";
  if (days < 30) return `Connected ${days} days ago`;
  return `Connected ${then.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    year: "numeric",
  })}`;
}
