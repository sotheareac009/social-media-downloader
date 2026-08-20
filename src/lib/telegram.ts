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
import { verifyPbkdf2, webcryptoPbkdf2 } from "@/lib/gramjsCryptoFile";
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

export const telegramGetSession = () =>
  invoke<string | null>("telegram_get_session");

const telegramSaveSession = (session: string) =>
  invoke<TelegramStatus>("telegram_save_session", { session });

export const telegramClearSession = () =>
  invoke<TelegramStatus>("telegram_clear_session");

const telegramSetDisplayName = (name: string) =>
  invoke<TelegramStatus>("telegram_set_display_name", { name });

// ------------------------------------------------------------- upload

export interface TelegramChat {
  /** Peer id as a string (BigInt-safe). */
  id: string;
  title: string;
  kind: "group" | "channel";
}

/** A connected client kept alive across upload actions, from the stored session. */
let sharedClient: TelegramClient | null = null;

async function connectedClient(): Promise<TelegramClient> {
  if (sharedClient && sharedClient.connected) return sharedClient;
  const config = await telegramGetConfig();
  const session = await telegramGetSession();
  if (!config.configured || !session) {
    throw new TelegramLoginError("Telegram isn't connected. Sign in on the Telegram page.");
  }
  const client = new TelegramClient(
    new StringSession(session),
    config.api_id,
    config.api_hash,
    { connectionRetries: 3, useWSS: true, timeout: 15, baseLogger: new Logger(LogLevel.NONE) },
  );
  await client.connect();
  sharedClient = client;
  return client;
}

/** Groups and channels the signed-in account belongs to, for the picker. */
export async function telegramListChats(): Promise<TelegramChat[]> {
  const client = await connectedClient();
  const dialogs = await client.getDialogs({ limit: 500 });
  const out: TelegramChat[] = [];
  for (const d of dialogs) {
    // Groups and channels only — skip 1:1 user chats.
    if (d.isGroup || d.isChannel) {
      out.push({
        id: String(d.id),
        title: d.title || d.name || "Untitled",
        kind: d.isChannel && !d.isGroup ? "channel" : "group",
      });
    }
  }
  return out;
}

const avatarCache = new Map<string, string | null>();

/** A small profile-photo URL for a chat, or null if it has none. Cached. */
export async function telegramChatAvatar(chatId: string): Promise<string | null> {
  if (avatarCache.has(chatId)) return avatarCache.get(chatId) ?? null;
  try {
    const client = await connectedClient();
    const buf = (await client.downloadProfilePhoto(chatId, { isBig: false })) as
      | Uint8Array
      | string
      | undefined;
    if (!buf || (typeof buf !== "string" && buf.length === 0)) {
      avatarCache.set(chatId, null);
      return null;
    }
    const bytes = typeof buf === "string" ? new TextEncoder().encode(buf) : new Uint8Array(buf);
    const url = URL.createObjectURL(new Blob([bytes], { type: "image/jpeg" }));
    avatarCache.set(chatId, url);
    return url;
  } catch {
    avatarCache.set(chatId, null);
    return null;
  }
}

/** Send a file (already loaded as bytes) to a chat, with an optional caption. */
export interface SendVideoMeta {
  width: number;
  height: number;
  duration: number;
}

export async function telegramSendFile(
  chatId: string,
  bytes: Uint8Array,
  fileName: string,
  caption: string,
  videoMeta?: SendVideoMeta | null,
): Promise<void> {
  const client = await connectedClient();
  const { CustomFile } = await import("telegram/client/uploads");

  // Upload the in-memory bytes ourselves, then send the resulting handle.
  //
  // Why not just pass the file to sendFile: GramJS's default upload path reads
  // large CustomFiles from a file *path* (empty in a browser → throws), and a
  // raw browser File is mishandled by _fileToMedia. Uploading via uploadFile
  // with maxBufferSize ≥ the file size forces the in-memory buffer path, which
  // works at any size we can hold in memory.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const toUpload = new CustomFile(fileName, bytes.length, "", bytes as any);
  const handle = await client.uploadFile({
    file: toUpload,
    workers: 1,
    maxBufferSize: bytes.length + 1024,
  });

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const attributes: any[] = [new Api.DocumentAttributeFilename({ fileName })];
  // Telegram needs the video's dimensions/duration or it renders the preview
  // with a broken aspect ratio. Present only for real videos.
  if (videoMeta && videoMeta.width > 0 && videoMeta.height > 0) {
    attributes.push(
      new Api.DocumentAttributeVideo({
        duration: Math.round(videoMeta.duration),
        w: videoMeta.width,
        h: videoMeta.height,
        supportsStreaming: true,
      }),
    );
  }

  await client.sendFile(chatId, {
    file: handle,
    caption: caption || undefined,
    forceDocument: false,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    attributes: attributes as any,
  });
}

// ------------------------------------------------------------- login flow

/** Raised when a step fails, carrying a message safe to show a person. */
export class TelegramLoginError extends Error {}

/**
 * Re-wrap the SRP byte fields in the *global* Buffer class.
 *
 * GramJS serialises bytes behind `if (!(data instanceof Buffer)) throw ...`,
 * which compares class identity against whatever `globalThis.Buffer` is. In a
 * webview, Buffer is polyfilled, and the crypto packages GramJS uses for
 * `createHash` carry their own copy of it - so `computeCheck` returns perfectly
 * good bytes belonging to a *different* Buffer class, and the serializer
 * rejects them with "Bytes or str expected, not Buffer".
 *
 * The phone and code steps never hit this because their byte fields come from
 * the connection layer, which uses the global Buffer already. Only the SRP
 * values are produced by the hashing path.
 *
 * Copying through `globalThis.Buffer.from` gives the serializer the exact class
 * it tests for. This is independent of how the bundler chunks anything, which
 * is why it is done here rather than by aligning module copies.
 */
function adoptGlobalBuffers<T extends object>(check: T): T {
  const B = (globalThis as { Buffer?: { from(v: Uint8Array): Uint8Array } })
    .Buffer;
  if (!B) return check;

  const target = check as unknown as Record<string, unknown>;
  for (const field of ["A", "M1"]) {
    const value = target[field];
    if (value instanceof Uint8Array && !(value instanceof (B as never))) {
      target[field] = B.from(value);
    }
  }
  return check;
}

/**
 * Confirm the 2FA hash will be computed with native WebCrypto.
 *
 * The *binding* is guaranteed at build time: Vite redirects GramJS's
 * `./CryptoFile` to `@/lib/gramjsCryptoFile` in both pipelines - the Rollup
 * resolver for production and the esbuild pre-bundler for dev. Getting only one
 * of those right is what made 2FA behave differently in the .dmg than locally.
 *
 * There is deliberately no runtime patch. `CryptoFile` exposes `pbkdf2Sync` as
 * a non-configurable getter, so neither assignment nor `defineProperty` can
 * replace it; the previous attempt threw into a `catch` and reported the
 * failure as a wrong password. Importing the original module here would also
 * pull the broken implementation back into the bundle.
 *
 * What remains worth checking at runtime is that WebCrypto is present and
 * produces the reference value, since that depends on the webview.
 */
async function ensurePasswordCrypto(): Promise<void> {
  await verifyPbkdf2(webcryptoPbkdf2);
}

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

    // Deliberately outside the try below. A broken PBKDF2 produces a wrong SRP
    // proof, Telegram answers "password invalid", and the user gets told their
    // correct password is wrong - which is exactly the bug this replaced. A
    // crypto fault must report itself as a crypto fault.
    try {
      await ensurePasswordCrypto();
    } catch (e) {
      throw e instanceof TelegramLoginError
        ? e
        : new TelegramLoginError(
            e instanceof Error && e.message
              ? e.message
              : "Password encryption is unavailable in this build.",
          );
    }

    try {
      const pwd = await client.invoke(new Api.account.GetPassword());
      const check = await computeCheck(pwd, password);
      await client.invoke(
        new Api.auth.CheckPassword({ password: adoptGlobalBuffers(check) }),
      );
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
  if (typeof e === "object" && e !== null) {
    // GramJS RPC errors carry the Telegram code on `errorMessage`, which is not
    // always reflected in `message`. Missing it meant real causes such as
    // SRP_ID_INVALID fell through to the generic fallback and were reported as
    // a wrong password.
    const o = e as {
      errorMessage?: unknown;
      code?: unknown;
      name?: unknown;
      message?: unknown;
    };
    const parts = [o.name, o.errorMessage, o.message, o.code]
      .filter((v) => v !== undefined && v !== null && v !== "")
      .map(String);
    if (parts.length > 0) return Array.from(new Set(parts)).join(" ");
  }
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
  if (msg.includes("SRP_ID_INVALID"))
    return new TelegramLoginError(
      "The password challenge expired. Go back and start the login again.",
    );
  if (msg.includes("SRP_PASSWORD_CHANGED"))
    return new TelegramLoginError(
      "The account password changed during login. Start again.",
    );
  if (msg.includes("FLOOD_WAIT"))
    return new TelegramLoginError("Too many attempts. Wait a while before trying again.");
  if (msg.includes("PHONE_NUMBER_BANNED"))
    return new TelegramLoginError("This phone number is banned from Telegram.");
  // An unrecognised failure is NOT the same as a wrong password, and saying so
  // sends people to change a password that was fine. Keep the plain-language
  // fallback, but carry the underlying reason so it can be acted on.
  const detail = msg.trim().slice(0, 200);
  return new TelegramLoginError(
    detail ? `${fallback} (${detail})` : fallback,
  );
}
