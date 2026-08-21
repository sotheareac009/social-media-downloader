import { Buffer as SharedBuffer } from "buffer";
import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles/theme.css";
import "./styles/app.css";
import "./styles/publish.css";

/**
 * Pin the global Buffer to ONE class.
 *
 * `vite-plugin-node-polyfills` injects
 * `globalThis.Buffer = globalThis.Buffer || <its own bundled Buffer>` into every
 * dependency chunk, and that copy is not the same class as the `buffer` package
 * used by create-hash / create-hmac / pbkdf2.
 *
 * GramJS guards its serializer with `data instanceof Buffer`, reading `Buffer`
 * as a free global at call time. With two classes in play that check rejects
 * genuine Buffers - "Bytes or str expected, not Buffer" - which the Telegram
 * login reported as a wrong 2FA password.
 *
 * This runs after every dependency chunk has initialised (their bodies execute
 * during import evaluation, this executes after), so the assignment wins. The
 * `safe-buffer` and shim aliases in vite.config.ts remove the duplicates at
 * build time; this guarantees the runtime identity regardless of chunking.
 */
(globalThis as { Buffer?: unknown }).Buffer = SharedBuffer;

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
