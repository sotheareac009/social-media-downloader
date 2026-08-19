import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { nodePolyfills } from "vite-plugin-node-polyfills";
import { fileURLToPath, URL } from "node:url";

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [
    react(),
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
    alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) },
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
