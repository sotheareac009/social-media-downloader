/**
 * Replacement for GramJS's `telegram/CryptoFile`.
 *
 * GramJS reaches for Node's `crypto` module in four places:
 *
 *   Password.js  pbkdf2Sync   (2FA / SRP)
 *   Helpers.js   randomBytes
 *   Helpers.js   createHash("sha1")
 *   Helpers.js   createHash("sha256")
 *
 * In a webview those come from `vite-plugin-node-polyfills`. The polyfill's
 * pure-JS `pbkdf2Sync` is the problem: 100,000 iterations of HMAC-SHA512 in
 * JavaScript, and the value it produced did not match Node's reference, so
 * Telegram rejected correct passwords with "password invalid".
 *
 * This module keeps the polyfill for everything else and routes only PBKDF2
 * through the webview's native WebCrypto, which does PBKDF2-SHA512 in C.
 *
 * WHY A MODULE RATHER THAN A MONKEY-PATCH. The previous approach assigned to
 * `CryptoFile.default.pbkdf2Sync` at runtime. `CryptoFile.js` builds its
 * default export with TypeScript's `__importStar`, which installs properties as
 * getters with no setter, so the assignment threw. It was inside a try/catch,
 * so it failed *silently* - and only in production, because Vite's dev-time
 * dependency optimiser produces a mutable object while the Rollup build does
 * not. Dev worked, the .dmg did not. Replacing the module removes the guesswork:
 * there is nothing to patch and nothing that can silently not apply.
 */
import nodeCrypto from "crypto";

/**
 * PBKDF2-SHA512 via WebCrypto.
 *
 * Returns a Promise. That is safe: GramJS's only caller is
 * `const hash3 = await pbkdf2sha512(...)` in `Password.js`.
 */
export async function webcryptoPbkdf2(
  password: ArrayBufferView | string,
  salt: ArrayBufferView | string,
  iterations: number,
  keylen: number,
  _digest?: string,
): Promise<Uint8Array> {
  const subtle = globalThis.crypto?.subtle;
  if (!subtle) {
    // Better to fail loudly than to hand Telegram a wrong hash and have the
    // user told their password is wrong.
    throw new Error(
      "WebCrypto is unavailable in this webview, so the two-step password cannot be verified.",
    );
  }

  const key = await subtle.importKey(
    "raw",
    toBytes(password) as BufferSource,
    "PBKDF2",
    false,
    ["deriveBits"],
  );
  const bits = await subtle.deriveBits(
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
}

/**
 * Marker so the caller can tell, at runtime, whether the function GramJS holds
 * is this one. Without it a "patched" flag can be true while GramJS still
 * dereferences the original - which is exactly how the previous bug hid.
 */
(webcryptoPbkdf2 as unknown as { __webcrypto?: boolean }).__webcrypto = true;

function toBytes(v: ArrayBufferView | string): Uint8Array {
  if (typeof v === "string") return new TextEncoder().encode(v);
  return new Uint8Array(
    (v.buffer as ArrayBuffer).slice(v.byteOffset, v.byteOffset + v.byteLength),
  );
}

/**
 * Prove the implementation matches Node's reference before it is trusted with a
 * real password. Vector produced by
 * `crypto.pbkdf2Sync("password", "salt", 1, 64, "sha512")`.
 */
const VECTOR =
  "867f70cf1ade02cff3752599a3a53dc4af34c7a669815ae5d513554e1c8cf252" +
  "c02d470a285a0501bad999bfe943c08f050235d7d68b1da55e63f73b60a57fce";

/**
 * Run the reference vector through `fn`.
 *
 * Takes the function as an argument on purpose: the caller passes the exact
 * reference GramJS will dereference, so this cannot pass while GramJS uses a
 * different implementation.
 */
export async function verifyPbkdf2(
  fn: (
    p: ArrayBufferView | string,
    s: ArrayBufferView | string,
    i: number,
    k: number,
    d?: string,
  ) => Promise<Uint8Array> | Uint8Array,
): Promise<void> {
  const out = await fn("password", "salt", 1, 64, "sha512");
  const hex = Array.from(new Uint8Array(out))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
  if (hex !== VECTOR) {
    throw new Error("PBKDF2 self-test failed; refusing to attempt a 2FA login.");
  }
}

// Same shape GramJS expects: a default export with these members.
const cryptoFile = {
  ...nodeCrypto,
  randomBytes: nodeCrypto.randomBytes,
  createHash: nodeCrypto.createHash,
  pbkdf2Sync: webcryptoPbkdf2,
};

export default cryptoFile;
