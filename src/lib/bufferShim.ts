/**
 * Replacement for `vite-plugin-node-polyfills/shims/buffer`.
 *
 * WHY. That shim bundles its own private copy of the Buffer implementation, so
 * `shim.Buffer !== require("buffer").Buffer`. The plugin injects
 *
 *     globalThis.Buffer = globalThis.Buffer || __buffer_polyfill
 *
 * into every dependency chunk, which made the *global* Buffer the shim's class,
 * while `create-hash`/`create-hmac`/`pbkdf2` produced values from the `buffer`
 * package's class.
 *
 * GramJS then rejected its own data:
 *
 *     if (!(data instanceof Buffer)) throw Error(`Bytes or str expected, not ${...}`)
 *
 * `instanceof` compares class identity, so two Buffer classes fail the check
 * even though both are Buffers. During a Telegram login that surfaced as
 * "That password wasn't accepted" - a correct 2FA password blamed for a
 * bundling problem.
 *
 * This module re-exports the `buffer` package while keeping the plugin's
 * contract that the DEFAULT export is the Buffer class itself, so there is one
 * Buffer identity across the whole graph.
 */
import { Buffer } from "buffer";

export * from "buffer";

/**
 * The shim also exposes `Blob` and `File`, which the `buffer` package does not.
 * Both exist natively in any modern webview, so pass those through rather than
 * dropping names something might import.
 */
export const Blob = globalThis.Blob;
export const File = globalThis.File;

// The plugin assigns this straight to `globalThis.Buffer`, so it must be the
// class, not the module namespace.
export default Buffer;
