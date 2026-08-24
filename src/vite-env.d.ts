/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** When "true"/"1", the Upload menu and upload entry points are hidden. */
  readonly VITE_HIDE_UPLOAD?: string;
  /** When "true"/"1", the Publishing section and its routes are hidden. */
  readonly VITE_HIDE_PUBLISHING?: string;
  /** When "true"/"1", the Auto-scroll section is hidden. */
  readonly VITE_HIDE_AUTOSCROLL?: string;
  /** Comma-separated Upload target ids to hide, e.g. "facebook,tiktok,x". */
  readonly VITE_UPLOAD_HIDDEN_TARGETS?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
