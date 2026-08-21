import type { Platform } from "@/lib/publish";

/**
 * A platform's initial in its brand colour.
 *
 * Deliberately not the real logos: shipping Meta's and ByteDance's marks in a
 * third-party tool is a trademark question, not a design one. A coloured
 * initial identifies the row just as well at this size.
 */
const MARKS: Record<Platform, { letter: string; color: string; label: string }> = {
  facebook: { letter: "f", color: "#1877f2", label: "Facebook" },
  instagram: { letter: "IG", color: "#c13584", label: "Instagram" },
  tiktok: { letter: "TT", color: "#010101", label: "TikTok" },
  youtube: { letter: "YT", color: "#ff0000", label: "YouTube" },
};

export function PlatformMark({ platform, size = 32 }: { platform: Platform; size?: number }) {
  const mark = MARKS[platform];
  return (
    <span
      className="pmark"
      style={{
        width: size,
        height: size,
        background: mark.color,
        fontSize: Math.round(size * 0.42),
      }}
      aria-hidden
    >
      {mark.letter}
    </span>
  );
}

export const platformLabel = (platform: Platform) => MARKS[platform].label;
