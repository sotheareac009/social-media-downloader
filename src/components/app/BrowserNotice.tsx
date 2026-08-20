/**
 * Shown when the UI is loaded in an ordinary browser instead of the app window.
 *
 * Everything real in this app lives in Rust and is reached over Tauri's IPC
 * bridge. A plain browser tab has no bridge, so every `invoke` rejects with
 * "Cannot read properties of undefined (reading 'invoke')" - and each feature
 * reports that in its own words: connectivity says you are offline, the engine
 * panel says yt-dlp is not installed, downloads simply fail.
 *
 * All of those are the same cause and none of them name it, which sends people
 * to check their wifi or reinstall yt-dlp. One honest message is better than
 * five misleading ones.
 */
export function BrowserNotice({ url }: { url: string }) {
  return (
    <div className="gate">
      <div className="gate__panel rise">
        <div className="gate__mark" aria-hidden>
          <svg
            width="20"
            height="20"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.75"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <rect x="2.5" y="4" width="19" height="14" rx="2.5" />
            <path d="M8 21h8M12 18v3" />
          </svg>
        </div>

        <h1 className="gate__title">Open the desktop app</h1>
        <p className="gate__lede">
          You're viewing <code>{url}</code> in a web browser. This page is only
          the interface — downloading, Telegram and account sign-in all run in
          the app's own process, which a browser tab cannot reach.
        </p>

        <p className="gate__lede" style={{ marginTop: 12 }}>
          Use the <strong>Social Media Management window</strong> that{" "}
          <code>npm run tauri dev</code> opens. If you closed it, run that
          command again — visiting this address will never work on its own.
        </p>

        <p className="gate__hint">
          Any "offline", "yt-dlp isn't installed" or failed-download message on
          this page is caused by the missing connection to the app, not by your
          network or your tools.
        </p>
      </div>
    </div>
  );
}
