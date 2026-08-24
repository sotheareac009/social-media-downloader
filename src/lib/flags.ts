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

/** VITE_HIDE_AUTOSCROLL=true hides the standalone Auto-scroll section. */
export const HIDE_AUTOSCROLL = truthy(import.meta.env.VITE_HIDE_AUTOSCROLL);

/** VITE_UPLOAD_HIDDEN_TARGETS is a comma-separated list of Upload-page target
 *  ids to leave out of the "Post to" row — e.g. "facebook,tiktok,x". Ids are
 *  the ones `upload_targets` returns: youtube, facebook, tiktok, x, telegram.
 *  Unset (the default) hides nothing, so every platform still shows. */
const idSet = (v?: string) =>
  new Set(
    (v ?? "")
      .split(",")
      .map((s) => s.trim().toLowerCase())
      .filter(Boolean),
  );

export const HIDDEN_UPLOAD_TARGETS = idSet(
  import.meta.env.VITE_UPLOAD_HIDDEN_TARGETS,
);

/** True when this build should not offer `id` as an upload destination. */
export const isUploadTargetHidden = (id: string) =>
  HIDDEN_UPLOAD_TARGETS.has(id.toLowerCase());
