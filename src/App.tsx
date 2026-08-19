import { useEffect, useState } from "react";
import { AccountsPage } from "@/pages/accounts/AccountsPage";
import { DownloadsPage } from "@/pages/downloads/DownloadsPage";
import { HomePage } from "@/pages/home/HomePage";
import { TelegramPage } from "@/pages/telegram/TelegramPage";
import { SettingsPage } from "@/pages/settings/SettingsPage";
import { FacebookPage } from "@/pages/facebook/FacebookPage";
import { UploadPage } from "@/pages/upload/UploadPage";
import { authGetAccounts, subscribeToAuthEvents } from "@/lib/auth";
import { ToastProvider } from "@/components/ui/Toast";
import {
  BoltIcon,
  DownloadIcon,
  HomeIcon,
  MoonIcon,
  SendIcon,
  SlidersIcon,
  SunIcon,
  UploadIcon,
  UsersIcon,
} from "@/components/ui/icons";

type Theme = "light" | "dark";
type Route = "home" | "accounts" | "downloads" | "upload" | "telegram" | "facebook" | "settings";
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
          facebookConnected={facebookConnected}
          onNavigate={setRoute}
          onToggleTheme={() => setTheme(theme === "dark" ? "light" : "dark")}
        />
        <main className="main">
          <div className="titlebar" data-tauri-drag-region />
          <div className="main__scroll">
            {route === "home" && <HomePage onNavigate={setRoute} />}
            {route === "downloads" && <DownloadsPage />}
            {route === "upload" && <UploadPage />}
            {route === "accounts" && <AccountsPage onNavigate={setRoute} />}
            {route === "telegram" && <TelegramPage />}
            {route === "facebook" && <FacebookPage />}
            {route === "settings" && <SettingsPage />}
          </div>
        </main>
      </div>
    </ToastProvider>
  );
}

function Sidebar({
  theme,
  route,
  facebookConnected,
  onNavigate,
  onToggleTheme,
}: {
  theme: Theme;
  route: Route;
  facebookConnected: boolean;
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
          className={`navitem ${route === "telegram" ? "navitem--active" : ""}`}
          type="button"
          onClick={() => onNavigate("telegram")}
        >
          <span className="navitem__icon">
            <SendIcon size={16} />
          </span>
          Telegram
        </button>
        {facebookConnected && (
          <button
            className={`navitem ${route === "facebook" ? "navitem--active" : ""}`}
            type="button"
            onClick={() => onNavigate("facebook")}
          >
            <span className="navitem__icon">
              <FacebookGlyph />
            </span>
            Facebook
          </button>
        )}
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
        <button
          className={`navitem ${route === "settings" ? "navitem--active" : ""}`}
          type="button"
          onClick={() => onNavigate("settings")}
        >
          <span className="navitem__icon">
            <SlidersIcon size={16} />
          </span>
          Settings
        </button>
      </nav>

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

function FacebookGlyph() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" aria-hidden>
      <path d="M14.1 22v-8.6h2.9l.44-3.36H14.1V7.9c0-.97.27-1.63 1.66-1.63h1.78V3.26c-.31-.04-1.37-.13-2.6-.13-2.57 0-4.33 1.57-4.33 4.45v2.48H7.7v3.36h2.9V22z" />
    </svg>
  );
}
