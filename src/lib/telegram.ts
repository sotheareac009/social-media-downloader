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

export interface TelegramConfig {
  configured: boolean;
  api_id: number;
  api_hash: string;
}

export interface TelegramStatus {
  connected: boolean;
  connected_at: number | null;
}

// ---------------------------------------------------------------- commands

export const telegramGetConfig = () =>
  invoke<TelegramConfig>("telegram_get_config");

export const telegramStatus = () => invoke<TelegramStatus>("telegram_status");

const telegramGetSession = () =>
  invoke<string | null>("telegram_get_session");

const telegramSaveSession = (session: string) =>
  invoke<TelegramStatus>("telegram_save_session", { session });

export const telegramClearSession = () =>
  invoke<TelegramStatus>("telegram_clear_session");

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
      { connectionRetries: 3, baseLogger: new Logger(LogLevel.NONE) },
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
    { connectionRetries: 2, baseLogger: new Logger(LogLevel.NONE) },
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
