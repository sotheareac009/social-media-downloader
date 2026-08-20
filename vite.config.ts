import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { nodePolyfills } from "vite-plugin-node-polyfills";
import { fileURLToPath, URL } from "node:url";
import type { Plugin } from "vite";

const host = process.env.TAURI_DEV_HOST;

/**
 * Redirect GramJS's `./CryptoFile` to our own implementation.
 *
 * GramJS calls `CryptoFile.default.pbkdf2Sync(..., "sha512")` for the 2FA/SRP
 * hash. The node polyfill's pure-JS PBKDF2 produced a value that did not match
 * Node's reference, so Telegram rejected correct passwords. Our replacement
 * routes PBKDF2 through native WebCrypto and passes everything else through.
 *
 * Done as a resolver rather than a `resolve.alias` entry because `Password.js`
 * imports it by the RELATIVE specifier `./CryptoFile`, which a bare-specifier
 * alias never sees. The importer check keeps this scoped to the telegram
 * package, so no other module's `./CryptoFile` could be captured by accident.
 */
function gramjsWebCryptoPbkdf2(): Plugin {
  const replacement = fileURLToPath(
    new URL("./src/lib/gramjsCryptoFile.ts", import.meta.url),
  );
  return {
    name: "gramjs-webcrypto-pbkdf2",
    enforce: "pre",
    resolveId(source, importer) {
      if (!importer) return null;
      const fromTelegram = importer.replace(/\\/g, "/").includes("/node_modules/telegram/");
      if (!fromTelegram) return null;
      if (source !== "./CryptoFile" && source !== "./CryptoFile.js") return null;
      return replacement;
    },
  };
}

/**
 * The same redirect for Vite's dependency pre-bundler.
 *
 * Needed as well as the Rollup resolver above, not instead of it: dev
 * pre-bundles the `telegram` package with esbuild, which resolves
 * `./CryptoFile` internally before any Rollup hook runs. Without this, dev and
 * production disagree - which is precisely the failure mode being fixed, where
 * 2FA behaved differently in the .dmg than on the developer's machine.
 *
 * A runtime patch is not an option: `CryptoFile` exposes `pbkdf2Sync` as a
 * non-configurable getter, so neither assignment nor `defineProperty` can
 * replace it.
 */
function gramjsWebCryptoPbkdf2PreBundle() {
  const replacement = fileURLToPath(
    new URL("./src/lib/gramjsCryptoFile.ts", import.meta.url),
  );
  return {
    name: "gramjs-webcrypto-pbkdf2-prebundle",
    setup(build: {
      onResolve: (
        opts: { filter: RegExp },
        cb: (a: { importer: string }) => { path: string } | undefined,
      ) => void;
    }) {
      build.onResolve({ filter: /^\.\/CryptoFile(\.js)?$/ }, (args) => {
        const from = args.importer.replace(/\\/g, "/");
        if (!from.includes("/node_modules/telegram/")) return undefined;
        return { path: replacement };
      });
    },
  };
}

export default defineConfig({
  plugins: [
    react(),
    gramjsWebCryptoPbkdf2(),
    // GramJS (Telegram MTProto) assumes a Node environment: it uses `Buffer`
    // and `process` as globals, which Vite does not provide in a browser
    // build. Without these shims the app compiles but throws at runtime the
    // moment the Telegram client is constructed.
    nodePolyfills({
      globals: { Buffer: true, process: true, global: true },
      protocolImports: true,
    }),
  ],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
      // Collapse `safe-buffer` onto `buffer`.
      //
      // The browserify crypto packages (create-hash, create-hmac, pbkdf2,
      // randombytes, cipher-base) depend on `safe-buffer`, which ships its OWN
      // copy of the Buffer class - `safe-buffer.Buffer !== buffer.Buffer`.
      //
      // GramJS then fails on its own output: `Helpers.createHash(...)` returns a
      // safe-buffer Buffer, and `serializeBytes` guards with
      // `data instanceof Buffer` against the polyfill's Buffer. Two classes, so
      // the check throws "Bytes or str expected, not Buffer2" - which the login
      // reported as a rejected 2FA password.
      //
      // safe-buffer is a thin wrapper over the same API, so pointing it at
      // `buffer` gives one Buffer identity across the whole graph.
      "safe-buffer": "buffer",
      // The plugin's own buffer shim bundles a THIRD copy of the Buffer class,
      // and it is the one that becomes `globalThis.Buffer`. Point it at a shim
      // of ours that re-exports the `buffer` package instead.
      "vite-plugin-node-polyfills/shims/buffer": fileURLToPath(
        new URL("./src/lib/bufferShim.ts", import.meta.url),
      ),
    },
    // Belt and braces: never let a second copy of these be resolved.
    dedupe: ["buffer", "safe-buffer"],
  },
  optimizeDeps: {
    esbuildOptions: {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      plugins: [gramjsWebCryptoPbkdf2PreBundle() as any],
    },
  },
  // Tauri expects a fixed port and fails if it is not available.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
});
