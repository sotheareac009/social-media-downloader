/**
 * Licence activation bridge.
 *
 * The key itself never comes back from Rust - only the verified facts about it.
 * That keeps a working key from being recoverable via a screenshot or the
 * devtools console.
 */
import { invoke } from "@tauri-apps/api/core";

export interface LicenseStatus {
  /** False in development builds, which are not gated. */
  enforced: boolean;
  activated: boolean;
  plan: string | null;
  /** Unix seconds; null for a perpetual licence. */
  expires_at: number | null;
  /** Short id to quote in support, e.g. "6a6c2619". */
  tag: string | null;
}

export const licenseStatus = () => invoke<LicenseStatus>("license_status");

export const licenseActivate = (key: string) =>
  invoke<LicenseStatus>("license_activate", { key });

export const licenseDeactivate = () =>
  invoke<LicenseStatus>("license_deactivate");

/** Copy for the specific ways a key can be refused. */
export function licenseMessage(code: string, fallback: string): string {
  switch (code) {
    case "license_malformed":
      return "That doesn't look like a licence key. Paste the whole key, including the SMD1 prefix.";
    case "license_invalid":
      return "That key isn't valid. Check it was copied in full, with no characters missing.";
    case "license_expired":
      return "That licence has expired. Renew it to keep using the app.";
    case "license_unsupported":
      return "That key needs a newer version of the app. Update, then try again.";
    case "license_not_configured":
      return "This build can't check licences. Contact support.";
    default:
      return fallback;
  }
}

export function formatExpiry(unixSeconds: number | null): string {
  if (unixSeconds === null) return "Never expires";
  const when = new Date(unixSeconds * 1000);
  const days = Math.ceil((when.getTime() - Date.now()) / 86_400_000);
  const date = when.toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
  if (days <= 0) return `Expired ${date}`;
  if (days <= 30) return `Expires ${date} · ${days} day${days === 1 ? "" : "s"} left`;
  return `Expires ${date}`;
}
