/** Build-time feature flags, set via env (baked in by Vite at build).
 *
 *  VITE_HIDE_UPLOAD=true hides the Upload menu item and its entry points, for
 *  builds that should ship as download-only. */
const truthy = (v?: string) => v === "true" || v === "1" || v === "yes";

export const HIDE_UPLOAD = truthy(import.meta.env.VITE_HIDE_UPLOAD);
