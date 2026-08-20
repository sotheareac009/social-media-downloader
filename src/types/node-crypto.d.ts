/**
 * Minimal ambient types for the `crypto` specifier.
 *
 * At build time `vite-plugin-node-polyfills` resolves this to a browser shim.
 * `@types/node` is deliberately not installed - this is a webview app, and
 * pulling in the full Node type surface would let genuinely-unavailable Node
 * APIs typecheck. Only the members GramJS actually reaches for are declared.
 */
declare module "crypto" {
  export function randomBytes(size: number): Uint8Array;
  export function createHash(algorithm: string): {
    update(data: unknown): unknown;
    digest(encoding?: string): unknown;
  };
  const crypto: Record<string, unknown> & {
    randomBytes: typeof randomBytes;
    createHash: typeof createHash;
  };
  export default crypto;
}
