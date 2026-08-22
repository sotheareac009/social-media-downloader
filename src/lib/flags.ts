/** Build-time feature flags, set via env (baked in by Vite at build).
 *
 *  VITE_HIDE_UPLOAD=true hides the Upload menu item and its entry points, for
 *  builds that should ship as download-only.
 *
 *  VITE_HIDE_PUBLISHING=true hides the whole Publishing section — Dashboard,
 *  Emulator accounts, Publish and its Settings — for builds that should not
 *  carry the emulator publisher. The provider behind it is not mounted either,
 *  so a hidden build never shells out to ldconsole or adb. */
const truthy = (v?: string) => v === "true" || v === "1" || v === "yes";

export const HIDE_UPLOAD = truthy(import.meta.env.VITE_HIDE_UPLOAD);
export const HIDE_PUBLISHING = truthy(import.meta.env.VITE_HIDE_PUBLISHING);
