import { useEffect, useState } from "react";
import { AccountsPage } from "@/pages/accounts/AccountsPage";
import { DownloadsPage } from "@/pages/downloads/DownloadsPage";
import { HomePage } from "@/pages/home/HomePage";
import { ToastProvider } from "@/components/ui/Toast";
import {
  BoltIcon,
  DownloadIcon,
  HomeIcon,
  MoonIcon,
  SlidersIcon,
  SunIcon,
  UsersIcon,
} from "@/components/ui/icons";

type Theme = "light" | "dark";
type Route = "home" | "accounts" | "downloads";
const THEME_KEY = "md.theme";

export default function App() {
  const [theme, setTheme] = useState<Theme>(
    () => (localStorage.getItem(THEME_KEY) as Theme) ?? "dark",
  );
  // Home is the landing page: it reports what's ready before you try to use
  // it, which is the difference between "nothing works" and "install yt-dlp".
  const [route, setRoute] = useState<Route>("home");

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
            {route === "downloads" && <DownloadsPage />}
            {route === "accounts" && <AccountsPage />}
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
          className={`navitem ${route === "accounts" ? "navitem--active" : ""}`}
          type="button"
          onClick={() => onNavigate("accounts")}
        >
          <span className="navitem__icon">
            <UsersIcon size={16} />
          </span>
          Accounts
        </button>

        <button className="navitem" type="button" disabled>
          <span className="navitem__icon">
            <SlidersIcon size={16} />
          </span>
          Settings
          <span className="navitem__soon">Soon</span>
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
