import { useEffect, useState } from "react";
import { AccountsPage } from "@/pages/accounts/AccountsPage";
import { DownloadsPage } from "@/pages/downloads/DownloadsPage";
import { HomePage } from "@/pages/home/HomePage";
import { TelegramPage } from "@/pages/telegram/TelegramPage";
import { FacebookPage } from "@/pages/facebook/FacebookPage";
import { UploadPage } from "@/pages/upload/UploadPage";
import { authGetAccounts, subscribeToAuthEvents } from "@/lib/auth";
import { netPing, type NetStatus } from "@/lib/net";
import { ToastProvider } from "@/components/ui/Toast";
import {
  BoltIcon,
  DownloadIcon,
  GlobeIcon,
  HomeIcon,
  MoonIcon,
  SunIcon,
  UploadIcon,
  UsersIcon,
} from "@/components/ui/icons";

type Theme = "light" | "dark";
type Route = "home" | "accounts" | "downloads" | "upload" | "telegram" | "facebook";
const THEME_KEY = "md.theme";

export default function App() {
  const [theme, setTheme] = useState<Theme>(
    () => (localStorage.getItem(THEME_KEY) as Theme) ?? "dark",
  );
  // Home is the landing page: it reports what's ready before you try to use
  // it, which is the difference between "nothing works" and "install yt-dlp".
  const [route, setRoute] = useState<Route>("home");
  // The Facebook menu item appears only once a Facebook account is connected.
  const [facebookConnected, setFacebookConnected] = useState(false);
  // The Upload page holds real work-in-progress (chosen files, titles, target
  // chats). Mount it on first visit and keep it alive — hidden, not unmounted —
  // so navigating away and back doesn't wipe what you were doing.
  const [uploadVisited, setUploadVisited] = useState(false);
  useEffect(() => {
    if (route === "upload") setUploadVisited(true);
  }, [route]);

  useEffect(() => {
    let alive = true;
    const check = () =>
      authGetAccounts()
        .then((list) => alive && setFacebookConnected(list.some((a) => a.provider === "facebook" && a.connected)))
        .catch(() => {});
    void check();
    // Keep the menu in sync when an account connects or disconnects.
    const pending = subscribeToAuthEvents({ onSuccess: check, onDisconnected: check });
    return () => {
      alive = false;
      void pending.then((un) => un());
    };
  }, []);

  useEffect(() => {
    if (route === "facebook" && !facebookConnected) setRoute("accounts");
  }, [route, facebookConnected]);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    // A UI preference, not a credential — localStorage is the right home.
    localStorage.setItem(THEME_KEY, theme);
  }, [theme]);

  return (
    <ToastProvider>
      <div className="app">
        <Sidebar
          theme={theme}
          route={route}
          onNavigate={setRoute}
          onToggleTheme={() => setTheme(theme === "dark" ? "light" : "dark")}
        />
        <main className="main">
          <div className="titlebar" data-tauri-drag-region />
          <div className="main__scroll">
            {route === "home" && <HomePage onNavigate={setRoute} />}
            {route === "downloads" && <DownloadsPage onNavigate={setRoute} />}
            {uploadVisited && (
              <div hidden={route !== "upload"}>
                <UploadPage />
              </div>
            )}
            {route === "accounts" && <AccountsPage onNavigate={setRoute} />}
            {route === "telegram" && <TelegramPage onBack={() => setRoute("accounts")} />}
            {route === "facebook" && <FacebookPage onBack={() => setRoute("accounts")} />}
          </div>
        </main>
      </div>
    </ToastProvider>
  );
}

function Sidebar({
  theme,
  route,
  onNavigate,
  onToggleTheme,
}: {
  theme: Theme;
  route: Route;
  onNavigate: (r: Route) => void;
  onToggleTheme: () => void;
}) {
  return (
    <aside className="sidebar">
      <div className="sidebar__brand" data-tauri-drag-region>
        <div className="sidebar__mark">
          <BoltIcon size={16} />
        </div>
        <div>
          <div className="sidebar__title">Media Downloader</div>
          <div className="sidebar__subtitle">Public media · Accounts</div>
        </div>
      </div>

      <nav className="sidebar__section">
        <div className="sidebar__label">Library</div>
        <button
          className={`navitem ${route === "home" ? "navitem--active" : ""}`}
          type="button"
          onClick={() => onNavigate("home")}
        >
          <span className="navitem__icon">
            <HomeIcon size={16} />
          </span>
          Home
        </button>
        <button
          className={`navitem ${route === "downloads" ? "navitem--active" : ""}`}
          type="button"
          onClick={() => onNavigate("downloads")}
        >
          <span className="navitem__icon">
            <DownloadIcon size={16} />
          </span>
          Downloads
        </button>
        <button
          className={`navitem ${route === "upload" ? "navitem--active" : ""}`}
          type="button"
          onClick={() => onNavigate("upload")}
        >
          <span className="navitem__icon">
            <UploadIcon size={16} />
          </span>
          Upload
        </button>
        <button
          className={`navitem ${route === "accounts" ? "navitem--active" : ""}`}
          type="button"
          onClick={() => onNavigate("accounts")}
        >
          <span className="navitem__icon">
            <UsersIcon size={16} />
          </span>
          Accounts
        </button>
      </nav>

      <NetIndicator />

      <div className="sidebar__footer">
        <span className="sidebar__phase">v0.1.0</span>
        <button
          className="iconbutton"
          type="button"
          onClick={onToggleTheme}
          aria-label={`Switch to ${theme === "dark" ? "light" : "dark"} theme`}
          title={`Switch to ${theme === "dark" ? "light" : "dark"} theme`}
        >
          {theme === "dark" ? <SunIcon size={15} /> : <MoonIcon size={15} />}
        </button>
      </div>
    </aside>
  );
}

/** Live internet status + ping, polled from the Rust backend. */
function NetIndicator() {
  const [net, setNet] = useState<NetStatus | null>(null);
  const [checking, setChecking] = useState(false);

  const probe = () => {
    setChecking(true);
    netPing()
      .then(setNet)
      .catch(() => setNet({ online: false, ms: null, host: null }))
      .finally(() => setChecking(false));
  };

  useEffect(() => {
    probe();
    // Re-probe on a timer, and immediately when the OS reports a change.
    const id = window.setInterval(probe, 15_000);
    const onChange = () => probe();
    window.addEventListener("online", onChange);
    window.addEventListener("offline", onChange);
    return () => {
      window.clearInterval(id);
      window.removeEventListener("online", onChange);
      window.removeEventListener("offline", onChange);
    };
  }, []);

  const online = net?.online === true;
  const ms = net?.ms ?? null;
  const quality = ms === null ? "" : ms < 80 ? "good" : ms < 200 ? "ok" : "slow";

  return (
    <button
      className={`netstat ${online ? "netstat--on" : "netstat--off"} ${checking ? "netstat--checking" : ""}`.trim()}
      type="button"
      onClick={probe}
      title={
        net === null
          ? "Checking connection…"
          : online
            ? `Online via ${net.host ?? "internet"} · ${ms} ms — click to re-check`
            : "No internet connection — click to re-check"
      }
    >
      <span className={`netstat__dot ${quality ? `netstat__dot--${quality}` : ""}`.trim()} />
      <GlobeIcon size={13} />
      <span className="netstat__text">
        {net === null
          ? "Checking…"
          : online
            ? `Online${ms !== null ? ` · ${ms} ms` : ""}`
            : "Offline"}
      </span>
    </button>
  );
}
