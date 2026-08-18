import type { ReactNode } from "react";
import type { ProviderId } from "@/lib/auth";

/**
 * Official brand marks, inlined as SVG. Each provider also contributes a
 * `--brand` colour that the card's top hairline and hover states pick up.
 */
export const BRAND_COLOR: Record<ProviderId, string> = {
  google: "#4285F4",
  facebook: "#0866FF",
  // TikTok's tile is black, so the card hairline uses its accent red instead —
  // black would read as "no accent" against either theme.
  tiktok: "#FE2C55",
  // The midpoint of Instagram's gradient, for the flat hairline accent.
  instagram: "#DD2A7B",
};

export function ProviderLogo({ provider }: { provider: ProviderId }) {
  return (
    <div
      className={`logo logo--${provider}`}
      style={{ ["--brand" as string]: BRAND_COLOR[provider] }}
      aria-hidden
    >
      {MARKS[provider]}
    </div>
  );
}

const MARKS: Record<ProviderId, ReactNode> = {
  google: <GoogleMark />,
  facebook: <FacebookMark />,
  tiktok: <TikTokMark />,
  instagram: <InstagramMark />,
};

function GoogleMark() {
  return (
    <svg width="20" height="20" viewBox="0 0 48 48">
      <path
        fill="#4285F4"
        d="M45.1 24.5c0-1.6-.1-2.7-.4-4H24v7.5h12.1c-.2 2-1.6 5-4.5 7l-.1.3 6.5 5 .5.1c4.1-3.8 6.6-9.4 6.6-15.9z"
      />
      <path
        fill="#34A853"
        d="M24 46c5.9 0 10.9-2 14.5-5.3l-6.9-5.4c-1.8 1.3-4.3 2.2-7.6 2.2-5.8 0-10.7-3.8-12.5-9.1l-.3.02-6.8 5.2-.1.3C7.9 41 15.4 46 24 46z"
      />
      <path
        fill="#FBBC05"
        d="M11.5 28.4c-.5-1.4-.7-2.9-.7-4.4s.3-3 .7-4.4v-.3l-6.9-5.3-.2.1A22 22 0 0 0 2 24c0 3.5.9 6.9 2.4 9.9l7.1-5.5z"
      />
      <path
        fill="#EB4335"
        d="M24 10.5c4.1 0 6.9 1.8 8.5 3.3l6.2-6C34.9 4.3 29.9 2 24 2 15.4 2 7.9 7 4.4 14.1l7.1 5.5c1.8-5.3 6.7-9.1 12.5-9.1z"
      />
    </svg>
  );
}

function FacebookMark() {
  return (
    <svg width="22" height="22" viewBox="0 0 24 24" fill="currentColor">
      <path d="M14.1 22v-8.6h2.9l.44-3.36H14.1V7.9c0-.97.27-1.63 1.66-1.63h1.78V3.26c-.31-.04-1.37-.13-2.6-.13-2.57 0-4.33 1.57-4.33 4.45v2.48H7.7v3.36h2.9V22z" />
    </svg>
  );
}

/**
 * The note glyph, drawn three times: cyan offset up-left, red offset
 * down-right, white on top. That offset is the mark's defining feature — a
 * single flat white note reads as a generic music icon, not as TikTok.
 */
function TikTokMark() {
  const note =
    "M16.6 5.82A4.28 4.28 0 0 1 15.54 3h-3.09v12.4a2.59 2.59 0 1 1-2.6-2.58c.27 0 .53.04.78.12V9.66a5.7 5.7 0 1 0 4.91 5.64V9.01a7.35 7.35 0 0 0 4.3 1.38V7.3a4.35 4.35 0 0 1-3.24-1.48z";
  return (
    <svg width="21" height="21" viewBox="0 0 24 24">
      <path d={note} fill="#25F4EE" transform="translate(-1.1 -0.9)" />
      <path d={note} fill="#FE2C55" transform="translate(1.1 0.9)" />
      <path d={note} fill="#FFFFFF" />
    </svg>
  );
}

/**
 * The glyph is a stroked outline, so the tile's gradient shows through it —
 * a filled mark on the gradient would read as a solid blob at 42px.
 */
function InstagramMark() {
  return (
    <svg width="21" height="21" viewBox="0 0 24 24" fill="none"
      stroke="currentColor" strokeWidth="2" strokeLinecap="round"
      strokeLinejoin="round">
      <rect x="3" y="3" width="18" height="18" rx="5.2" />
      <circle cx="12" cy="12" r="4.1" />
      <circle cx="17.2" cy="6.8" r="1.15" fill="currentColor" stroke="none" />
    </svg>
  );
}
