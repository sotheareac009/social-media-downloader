import { useEffect, useRef, useState, type ReactElement } from "react";
import {
  downloadEngineStatus,
  instagramStatus,
  type EngineStatus,
  type SessionStatus,
} from "@/lib/download";
import { authGetAccounts, type AccountView } from "@/lib/auth";
import { SourceLogo, SOURCE_COLOR, type SourceId } from "@/components/home/SourceLogo";
import {
  BoltIcon,
  DownloadIcon,
  ShieldIcon,
  SlidersIcon,
  UsersIcon,
  type IconProps,
} from "@/components/ui/icons";

export type Route = "home" | "downloads" | "accounts";

type Tone = "ok" | "warn" | "muted";

interface Platform {
  id: SourceId;
  name: string;
  /** Exactly what this build handles — no aspirational entries. */
  supports: string[];
  /** Live state, for the one platform whose availability can change. */
  note?: { label: string; tone: Tone };
}

/**
 * The platforms the downloader actually supports.
 *
 * Google is absent on purpose: it is an Accounts-page sign-in only and has no
 * download path, so listing it would promise something that does not exist.
 */
const PLATFORMS: Platform[] = [
  {
    id: "youtube",
    name: "YouTube",
    supports: ["Videos", "Shorts", "Channels & playlists", "Up to 8K"],
  },
  {
    id: "tiktok",
    name: "TikTok",
    supports: ["Videos", "Whole profiles", "Photo posts"],
  },
  {
    id: "facebook",
    name: "Facebook",
    supports: ["Videos", "Reels", "Share links"],
  },
  {
    id: "instagram",
    name: "Instagram",
    supports: ["Reels", "Whole profiles", "Posts & IGTV", "Sign-in required"],
  },
];

interface Feature {
  key: string;
  title: string;
  blurb: string;
  icon: (p: IconProps) => ReactElement;
  goto?: Route;
  status: { label: string; tone: Tone };
}

export function HomePage({ onNavigate }: { onNavigate: (r: Route) => void }) {
  const [engine, setEngine] = useState<EngineStatus | null>(null);
  const [accounts, setAccounts] = useState<AccountView[] | null>(null);
  const [instagram, setInstagram] = useState<SessionStatus | null>(null);

  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  useEffect(() => {
    void (async () => {
      const [e, a, ig] = await Promise.allSettled([
        downloadEngineStatus(),
        authGetAccounts(),
        instagramStatus(),
      ]);
      if (!mounted.current) return;
      if (e.status === "fulfilled") setEngine(e.value);
      if (a.status === "fulfilled") setAccounts(a.value);
      if (ig.status === "fulfilled") setInstagram(ig.value);
    })();
  }, []);

  const connected = accounts?.filter((a) => a.connected).length ?? 0;

  const features: Feature[] = [
    {
      key: "bulk",
      title: "Whole profiles & channels",
      blurb:
        "Paste a TikTok profile or YouTube channel and queue everything they've posted. You see the count and confirm before anything downloads.",
      icon: BoltIcon,
      goto: "downloads",
      status: { label: "TikTok & YouTube", tone: "muted" },
    },
    {
      key: "quality",
      title: "Quality, read from the video",
      blurb:
        "Every link is inspected and offers the resolutions it actually has — up to 8K when the video carries it.",
      icon: SlidersIcon,
      goto: "downloads",
      status: engine
        ? engine.has_ffmpeg
          ? { label: "FFmpeg ready", tone: "ok" }
          : { label: "Capped at 360p — install FFmpeg", tone: "warn" }
        : { label: "Checking…", tone: "muted" },
    },
    {
      key: "accounts",
      title: "Connected accounts",
      blurb:
        "Sign in with your own account. Tokens live in your keychain, and signing in never unlocks private posts.",
      icon: UsersIcon,
      goto: "accounts",
      status:
        connected > 0
          ? { label: `${connected} connected`, tone: "ok" }
          : { label: "Optional", tone: "muted" },
    },
    {
      key: "private",
      title: "Sessions stay narrow",
      blurb:
        "YouTube, Facebook and TikTok download with no session at all. Instagram needs a login, and uses only the one you sign into here — never your browser profile.",
      icon: ShieldIcon,
      status: { label: "Always on", tone: "ok" },
    },
  ];

  const ready = engine?.available === true;

  return (
    <div className="page">
      <header className="hero rise">
        <span className="hero__eyebrow">
          <BoltIcon size={12} />
          Media Downloader
        </span>
        <h1 className="hero__title">
          Save any <span className="hero__accent">public video</span>
          <br />
          in a couple of clicks.
        </h1>
        <p className="hero__lede">
          YouTube, TikTok and Facebook — single links, whole profiles, or a
          pasted list. No account required.
        </p>
        <div className="hero__actions">
          <button
            className="btn btn--primary hero__cta"
            type="button"
            onClick={() => onNavigate("downloads")}
          >
            <DownloadIcon size={15} />
            Start downloading
          </button>
          <span className={`hero__pill hero__pill--${ready ? "ok" : "warn"}`}>
            <span className="hero__dot" />
            {engine === null
              ? "Checking engine…"
              : ready
                ? `Engine ready${engine.version ? ` · yt-dlp ${engine.version}` : ""}`
                : "yt-dlp not installed"}
          </span>
        </div>
      </header>

      <section className="section rise" style={{ animationDelay: "60ms" }}>
        <h2 className="section__label">Supported platforms</h2>
        <div className="platforms">
          {PLATFORMS.map((raw, i) => {
            const p: Platform =
              raw.id === "instagram"
                ? {
                    ...raw,
                    note: instagram?.connected
                      ? { label: "Signed in", tone: "ok" }
                      : { label: "Needs sign-in", tone: "warn" },
                  }
                : raw;
            return (
              <button
                key={p.id}
                type="button"
                className="platform"
                style={{
                  ["--brand" as string]: SOURCE_COLOR[p.id],
                  animationDelay: `${i * 60}ms`,
                }}
                onClick={() => onNavigate("downloads")}
              >
                <span className="platform__edge" />
                <SourceLogo source={p.id} />
                <span className="platform__name">{p.name}</span>
                <ul className="platform__list">
                  {p.supports.map((s) => (
                    <li key={s}>{s}</li>
                  ))}
                </ul>
                {p.note && (
                  <span className={`platform__note platform__note--${p.note.tone}`}>
                    {p.note.label}
                  </span>
                )}
              </button>
            );
          })}
        </div>
      </section>

      <section className="section rise" style={{ animationDelay: "120ms" }}>
        <h2 className="section__label">What it does</h2>
        <div className="features">
          {features.map((f, i) => (
            <FeatureCard key={f.key} feature={f} index={i} onNavigate={onNavigate} />
          ))}
        </div>
      </section>
    </div>
  );
}

function FeatureCard({
  feature,
  index,
  onNavigate,
}: {
  feature: Feature;
  index: number;
  onNavigate: (r: Route) => void;
}) {
  const Icon = feature.icon;
  const clickable = feature.goto !== undefined;

  return (
    <button
      type="button"
      className={`feature ${clickable ? "" : "feature--inert"}`.trim()}
      style={{ animationDelay: `${index * 50}ms` }}
      onClick={() => feature.goto && onNavigate(feature.goto)}
      disabled={!clickable}
    >
      <span className="feature__icon">
        <Icon size={18} />
      </span>
      <span className="feature__title">{feature.title}</span>
      <span className="feature__blurb">{feature.blurb}</span>
      <span className={`feature__status feature__status--${feature.status.tone}`}>
        {feature.status.label}
      </span>
    </button>
  );
}
