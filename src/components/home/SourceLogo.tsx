import { ProviderLogo } from "@/components/accounts/ProviderLogo";

/** Platforms this build can fetch public media from. */
export type SourceId = "youtube" | "tiktok" | "facebook" | "instagram" | "telegram";

export const SOURCE_COLOR: Record<SourceId, string> = {
  youtube: "#FF0033",
  tiktok: "#FE2C55",
  facebook: "#0866FF",
  // The midpoint of Instagram's gradient, matching the Accounts page.
  instagram: "#DD2A7B",
  // Telegram brand blue (the midpoint of its #2AABEE→#229ED9 gradient).
  telegram: "#229ED9",
};

/**
 * Brand tile for a download source.
 *
 * Facebook and TikTok already have marks on the Accounts page, so those are
 * reused rather than redrawn — two copies of a logo drift apart. YouTube is
 * not an auth provider, so it needs its own.
 */
export function SourceLogo({ source }: { source: SourceId }) {
  if (source === "youtube") {
    return (
      <div
        className="logo logo--youtube"
        style={{ ["--brand" as string]: SOURCE_COLOR.youtube }}
        aria-hidden
      >
        <svg width="22" height="22" viewBox="0 0 24 24" fill="currentColor">
          <path d="M10 8.64 15.27 12 10 15.36V8.64Z" />
        </svg>
      </div>
    );
  }
  if (source === "telegram") {
    // Its own tile: ProviderLogo has no Telegram mark and would render blank.
    return (
      <div
        className="logo logo--telegram"
        style={{ ["--brand" as string]: SOURCE_COLOR.telegram }}
        aria-hidden
      >
        <TelegramMark />
      </div>
    );
  }
  return <ProviderLogo provider={source} />;
}

/** The Telegram paper plane, white, as it sits on the app's blue circle. */
export function TelegramMark({ size = 21 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="#fff" aria-hidden>
      <path d="M21.94 4.4 18.9 19.1c-.23 1.02-.84 1.27-1.7.79l-4.7-3.47-2.27 2.19c-.25.25-.46.46-.94.46l.34-4.8L18.4 6.9c.38-.34-.08-.53-.6-.19L6.98 13.7l-4.64-1.45c-1.01-.32-1.03-1.01.21-1.5l18.15-7c.84-.3 1.58.2 1.24 1.65Z" />
    </svg>
  );
}
