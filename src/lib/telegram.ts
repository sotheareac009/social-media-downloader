/**
 * Telegram login, via GramJS.
 *
 * MTProto is not something this app reimplements, so login runs here in the
 * webview through GramJS and hands Rust only the resulting session string to
 * persist (see `src-tauri/src/telegram.rs`). The flow is a small state
 * machine, because the three steps — phone, code, optional 2FA — are separate
 * user interactions and GramJS's own `signInUser` helper drives them through
 * callbacks that don't map onto a UI.
 */
import { invoke } from "@tauri-apps/api/core";
import { TelegramClient, Api } from "telegram";
import { StringSession } from "telegram/sessions";
import { computeCheck } from "telegram/Password";
import { Logger } from "telegram/extensions";
import { LogLevel } from "telegram/extensions/Logger";

/**
 * Route GramJS's one PBKDF2 call through the webview's native WebCrypto.
 *
 * GramJS computes the 2FA (SRP) password hash with `crypto.pbkdf2Sync(…,
 * "sha512")`. WKWebView's `crypto.subtle` does PBKDF2-SHA512 natively and is
 * verified byte-identical to Node's reference, so this removes any doubt about
 * the bundled pure-JS pbkdf2. GramJS `await`s the result, so returning a
 * Promise is fine.
 *
 * Done lazily and defensively: a dynamic import inside a try/catch, run only
 * just before a 2FA check. Nothing here touches module load, so a failure to
 * patch can never blank the app — at worst 2FA falls back to GramJS's own
 * implementation.
 */
let pbkdf2Patched = false;
async function ensurePbkdf2Patched(): Promise<void> {
  if (pbkdf2Patched) return;
  try {
    const mod = await import("telegram/CryptoFile");
    const cf = (mod.default ?? mod) as { pbkdf2Sync?: unknown };
    cf.pbkdf2Sync = async (
      password: ArrayBufferView | string,
      salt: ArrayBufferView | string,
      iterations: number,
      keylen: number,
    ): Promise<Uint8Array> => {
      const toBytes = (v: ArrayBufferView | string): Uint8Array =>
        typeof v === "string"
          ? new TextEncoder().encode(v)
          : new Uint8Array(
              (v.buffer as ArrayBuffer).slice(
                v.byteOffset,
                v.byteOffset + v.byteLength,
              ),
            );

      const key = await crypto.subtle.importKey(
        "raw",
        toBytes(password) as BufferSource,
        "PBKDF2",
        false,
        ["deriveBits"],
      );
      const bits = await crypto.subtle.deriveBits(
        {
          name: "PBKDF2",
          salt: toBytes(salt) as BufferSource,
          iterations,
          hash: "SHA-512",
        },
        key,
        keylen * 8,
      );
      return new Uint8Array(bits);
    };
    pbkdf2Patched = true;
  } catch {
    // Leave GramJS's own pbkdf2 in place; the app must still work.
  }
}

export interface TelegramConfig {
  configured: boolean;
  api_id: number;
  api_hash: string;
}

export interface TelegramStatus {
  connected: boolean;
  connected_at: number | null;
  display_name: string | null;
}

// ---------------------------------------------------------------- commands

export const telegramGetConfig = () =>
  invoke<TelegramConfig>("telegram_get_config");

export const telegramSetConfig = (apiId: string, apiHash: string) =>
  invoke<TelegramConfig>("telegram_set_config", { apiId, apiHash });

export const telegramClearConfig = () =>
  invoke<TelegramConfig>("telegram_clear_config");

export const telegramStatus = () => invoke<TelegramStatus>("telegram_status");

const telegramGetSession = () =>
  invoke<string | null>("telegram_get_session");

const telegramSaveSession = (session: string) =>
  invoke<TelegramStatus>("telegram_save_session", { session });

export const telegramClearSession = () =>
  invoke<TelegramStatus>("telegram_clear_session");

const telegramSetDisplayName = (name: string) =>
  invoke<TelegramStatus>("telegram_set_display_name", { name });

// ------------------------------------------------------------- login flow

/** Raised when a step fails, carrying a message safe to show a person. */
export class TelegramLoginError extends Error {}

/**
 * A login attempt in progress.
 *
 * Holds the one live client across steps. `connect()` opens the socket and
 * sends the code; `submitCode` and `submitPassword` advance it. The client is
 * disconnected and dropped once a session is saved or the attempt is abandoned.
 */
export class TelegramLogin {
  private client: TelegramClient | null = null;
  private phoneNumber = "";
  private phoneCodeHash = "";

  constructor(private readonly config: TelegramConfig) {}

  /** Open the connection and send the login code to `phoneNumber`. */
  async start(phoneNumber: string): Promise<void> {
    this.phoneNumber = phoneNumber.trim();
    if (!/^\+?\d[\d\s]{5,}$/.test(this.phoneNumber)) {
      throw new TelegramLoginError("Enter a phone number in full, with country code.");
    }

    // Silence GramJS's very chatty default logger.
    const client = new TelegramClient(
      new StringSession(""),
      this.config.api_id,
      this.config.api_hash,
      {
        connectionRetries: 3,
        useWSS: true,
        // Give up rather than hang forever when a DC can't be reached.
        timeout: 15,
        baseLogger: new Logger(LogLevel.NONE),
      },
    );

    try {
      await client.connect();
      const { phoneCodeHash } = await client.sendCode(
        { apiId: this.config.api_id, apiHash: this.config.api_hash },
        this.phoneNumber,
      );
      this.client = client;
      this.phoneCodeHash = phoneCodeHash;
    } catch (e) {
      await safeDisconnect(client);
      throw asLoginError(e, "Couldn't send the login code.");
    }
  }

  /**
   * Submit the code Telegram sent. Resolves `"done"` when signed in, or
   * `"password"` when a 2FA password is still required.
   */
  async submitCode(code: string): Promise<"done" | "password"> {
    const client = this.expectClient();
    try {
      await client.invoke(
        new Api.auth.SignIn({
          phoneNumber: this.phoneNumber,
          phoneCodeHash: this.phoneCodeHash,
          phoneCode: code.trim(),
        }),
      );
      await this.persist();
      return "done";
    } catch (e) {
      // The one non-error outcome: the account has 2FA on.
      if (messageOf(e).includes("SESSION_PASSWORD_NEEDED")) {
        return "password";
      }
      throw asLoginError(e, "That code wasn't accepted.");
    }
  }

  /** Complete a 2FA login with the cloud password. */
  async submitPassword(password: string): Promise<"done"> {
    const client = this.expectClient();
    try {
      await ensurePbkdf2Patched();
      const pwd = await client.invoke(new Api.account.GetPassword());
      const check = await computeCheck(pwd, password);
      await client.invoke(new Api.auth.CheckPassword({ password: check }));
      await this.persist();
      return "done";
    } catch (e) {
      throw asLoginError(e, "That password wasn't accepted.");
    }
  }

  /** Abandon an in-progress login and release the socket. */
  async cancel(): Promise<void> {
    if (this.client) {
      await safeDisconnect(this.client);
      this.client = null;
    }
  }

  private async persist(): Promise<void> {
    const client = this.expectClient();
    // `session.save()` on a StringSession returns the serialized string.
    const session = (client.session.save() as unknown as string) ?? "";
    if (!session) {
      throw new TelegramLoginError("Signed in, but no session was produced.");
    }
    await telegramSaveSession(session);

    // Best-effort: record the account's display name for the Accounts list.
    try {
      const me = await client.getMe();
      const name =
        [me.firstName, me.lastName].filter(Boolean).join(" ") ||
        me.username ||
        "Telegram account";
      await telegramSetDisplayName(name);
    } catch {
      /* the session is saved; a missing name is cosmetic */
    }

    await safeDisconnect(client);
    this.client = null;
  }

  private expectClient(): TelegramClient {
    if (!this.client) {
      throw new TelegramLoginError("Start the login again — the connection was lost.");
    }
    return this.client;
  }
}

/**
 * Reconnect with the stored session and confirm it still works, without any
 * user interaction. Returns false when there is no session or it has expired.
 */
export async function telegramValidateSession(
  config: TelegramConfig,
): Promise<boolean> {
  const saved = await telegramGetSession();
  if (!saved) return false;

  const client = new TelegramClient(
    new StringSession(saved),
    config.api_id,
    config.api_hash,
    { connectionRetries: 2, useWSS: true, timeout: 15, baseLogger: new Logger(LogLevel.NONE) },
  );
  try {
    await client.connect();
    const authorized = await client.isUserAuthorized();
    return authorized;
  } catch {
    return false;
  } finally {
    await safeDisconnect(client);
  }
}

// ------------------------------------------------------------------ util

async function safeDisconnect(client: TelegramClient): Promise<void> {
  try {
    await client.disconnect();
  } catch {
    /* closing a socket that's already gone is not worth surfacing */
  }
}

function messageOf(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

/** Map GramJS's RPC error codes to sentences a person can act on. */
function asLoginError(e: unknown, fallback: string): TelegramLoginError {
  const msg = messageOf(e);
  if (msg.includes("PHONE_NUMBER_INVALID"))
    return new TelegramLoginError("That phone number isn't valid.");
  if (msg.includes("PHONE_CODE_INVALID") || msg.includes("PHONE_CODE_EMPTY"))
    return new TelegramLoginError("That code is incorrect.");
  if (msg.includes("PHONE_CODE_EXPIRED"))
    return new TelegramLoginError("That code expired. Start again to get a new one.");
  if (msg.includes("PASSWORD_HASH_INVALID"))
    return new TelegramLoginError("That 2FA password is incorrect.");
  if (msg.includes("FLOOD_WAIT"))
    return new TelegramLoginError("Too many attempts. Wait a while before trying again.");
  if (msg.includes("PHONE_NUMBER_BANNED"))
    return new TelegramLoginError("This phone number is banned from Telegram.");
  return new TelegramLoginError(fallback);
}
