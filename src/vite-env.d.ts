/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** When "true"/"1", the Upload menu and upload entry points are hidden. */
  readonly VITE_HIDE_UPLOAD?: string;
  /** When "true"/"1", the Publishing section and its routes are hidden. */
  readonly VITE_HIDE_PUBLISHING?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
