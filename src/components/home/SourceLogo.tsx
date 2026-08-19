import { ProviderLogo } from "@/components/accounts/ProviderLogo";

/** Platforms this build can fetch public media from. */
export type SourceId = "youtube" | "tiktok" | "facebook" | "instagram";

export const SOURCE_COLOR: Record<SourceId, string> = {
  youtube: "#FF0033",
  tiktok: "#FE2C55",
  facebook: "#0866FF",
  // The midpoint of Instagram's gradient, matching the Accounts page.
  instagram: "#DD2A7B",
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
  return <ProviderLogo provider={source} />;
}
