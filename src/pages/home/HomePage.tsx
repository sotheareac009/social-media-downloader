import { useEffect, useRef, useState, type ReactElement } from "react";
import {
  instagramStatus,
  type SessionStatus,
} from "@/lib/download";
import { authGetAccounts, type AccountView } from "@/lib/auth";
import { useEngineStatus } from "@/components/ui/EngineStatusProvider";
import { SourceLogo, SOURCE_COLOR, type SourceId } from "@/components/home/SourceLogo";
import { HIDE_UPLOAD } from "@/lib/flags";
import {
  BoltIcon,
  DownloadIcon,
  ShieldIcon,
  SlidersIcon,
  UploadIcon,
  UsersIcon,
  type IconProps,
} from "@/components/ui/icons";

export type Route = "home" | "downloads" | "accounts" | "telegram" | "upload";

type Tone = "ok" | "warn" | "muted";

/** A download/upload capability line: what it does + whether sign-in is needed.
 *  `login`: "no" = works logged-out, "yes" = sign-in required, "some" = public
 *  works logged-out but more (private/profiles) needs sign-in. */
interface Capability {
  text: string;
  login: "no" | "yes" | "some";
}

interface Platform {
  id: SourceId;
  name: string;
  /** What this platform can download; omitted when it has no download path. */
  download?: Capability;
  /** What this platform can upload/post; omitted when it has none. */
  upload?: Capability;
  /** Where the card leads. Most go to Downloads; Telegram to its own page. */
  goto: Route;
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
    download: { text: "Videos, Shorts, channels — up to 8K", login: "no" },
    upload: { text: "Sign in to post to your channel", login: "yes" },
    goto: "downloads",
  },
  {
    id: "tiktok",
    name: "TikTok",
    download: { text: "Videos, whole profiles, photos", login: "no" },
    upload: { text: "Sign in to post (after app review)", login: "yes" },
    goto: "downloads",
  },
  {
    id: "facebook",
    name: "Facebook",
    download: { text: "Videos, reels & share links", login: "some" },
    upload: { text: "Sign in to post a photo to a Page", login: "yes" },
    goto: "downloads",
  },
  {
    id: "instagram",
    name: "Instagram",
    download: { text: "Reels, posts & whole profiles", login: "yes" },
    goto: "downloads",
  },
  {
    id: "x",
    name: "X",
    download: { text: "Posts, videos & whole profiles", login: "some" },
    upload: { text: "Sign in to post (needs X API credits)", login: "yes" },
    goto: "downloads",
  },
  {
    id: "telegram",
    name: "Telegram",
    upload: { text: "Sign in by phone to send to groups & channels", login: "yes" },
    goto: "telegram",
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

function CapRow({
  icon,
  kind,
  cap,
}: {
  icon: ReactElement;
  kind: string;
  cap: Capability;
}) {
  const label =
    cap.login === "no" ? "No login" : cap.login === "yes" ? "Sign-in" : "Public + sign-in";
  return (
    <div className={`cap cap--${kind.toLowerCase()}`}>
      <div className="cap__head">
        <span className="cap__icon">{icon}</span>
        <span className="cap__kind">{kind}</span>
        <span className={`cap__login cap__login--${cap.login}`}>{label}</span>
      </div>
      <p className="cap__text">{cap.text}</p>
    </div>
  );
}

export function HomePage({ onNavigate }: { onNavigate: (r: Route) => void }) {
  // Shared with the Downloads page: one probe per launch, not one per visit.
  const { engine } = useEngineStatus();
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
      const [a, ig] = await Promise.allSettled([
        authGetAccounts(),
        instagramStatus(),
      ]);
      if (!mounted.current) return;
      if (a.status === "fulfilled") setAccounts(a.value);
      if (ig.status === "fulfilled") setInstagram(ig.value);
    })();
  }, []);

  const connected = accounts?.filter((a) => a.connected).length ?? 0;

  const features: Feature[] = [
    {
      key: "yt-multi",
      title: "Publish to several YouTube channels at once",
      blurb:
        "Sign in to more than one Google account on the Upload page, tick the channels you want, and send the same video to all of them in a single upload — each with its own title, description and visibility.",
      icon: UploadIcon,
      goto: "upload",
      status: { label: "New", tone: "ok" },
    },
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
          SocialSync
        </span>
        <h1 className="hero__title">
          Download and <span className="hero__accent">publish</span>
          <br />
          across your platforms.
        </h1>
        <p className="hero__lede">
          Download from YouTube, TikTok, Facebook, Instagram and X — single
          links, whole profiles, or a pasted list. Then sign in to publish your
          own videos to YouTube, Telegram and X. Public downloads need no
          account.
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
          {!HIDE_UPLOAD && (
            <button
              className="btn btn--ghost hero__cta"
              type="button"
              onClick={() => onNavigate("upload")}
            >
              <UploadIcon size={15} />
              Start uploading
            </button>
          )}
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
                onClick={() => onNavigate(p.goto)}
              >
                <span className="platform__edge" />
                <SourceLogo source={p.id} />
                <span className="platform__name">{p.name}</span>
                <div className="platform__caps">
                  {p.download && (
                    <CapRow icon={<DownloadIcon size={13} />} kind="Download" cap={p.download} />
                  )}
                  {p.upload && (
                    <CapRow icon={<UploadIcon size={13} />} kind="Upload" cap={p.upload} />
                  )}
                </div>
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
          {features.filter((f) => !(HIDE_UPLOAD && f.goto === "upload")).map((f, i) => (
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
