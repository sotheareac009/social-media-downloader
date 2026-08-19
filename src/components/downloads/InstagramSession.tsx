import { Button } from "@/components/ui/Button";
import { CheckIcon, ShieldIcon, XIcon } from "@/components/ui/icons";
import type { SessionStatus } from "@/lib/download";

/**
 * Instagram is the one source that needs a login.
 *
 * The panel states plainly what connecting does and what it costs, because
 * this is the only place in the app where a download uses a session — and an
 * Instagram session cookie is far more powerful than the profile-scoped tokens
 * on the Accounts page.
 */
export function InstagramSessionCard({
  status,
  busy,
  onConnect,
  onDisconnect,
}: {
  status: SessionStatus;
  busy: boolean;
  onConnect: () => void;
  onDisconnect: () => void;
}) {
  return (
    <div className={`igsession ${status.connected ? "igsession--on" : ""}`.trim()}>
      <span className="igsession__icon">
        {status.connected ? <CheckIcon size={14} /> : <ShieldIcon size={14} />}
      </span>
      <div className="igsession__body">
        <div className="igsession__title">
          Instagram{" "}
          {status.connected ? (
            <span className="igsession__tag">Signed in</span>
          ) : (
            <span className="igsession__tag igsession__tag--off">Sign-in required</span>
          )}
        </div>
        <p className="igsession__lede">
          {status.connected ? (
            <>
              Reels and posts will download using the session you signed into.
              It's kept in an owner-only file in this app's data folder, and
              sent only to Instagram.
            </>
          ) : (
            <>
              Instagram blocks anonymous downloads, so reels need a login. This
              opens a window just for Instagram — your other browser sessions
              are never read.
            </>
          )}
        </p>
        {!status.connected && (
          <p className="igsession__warn">
            Instagram counts these downloads against your own account and can
            flag one that fetches heavily. Everything else here — YouTube,
            TikTok, Facebook — still downloads with no session at all.
          </p>
        )}
      </div>
      <div className="igsession__actions">
        {status.connected ? (
          <Button variant="ghost" className="btn--sm" onClick={onDisconnect} disabled={busy}>
            <XIcon size={13} />
            Sign out
          </Button>
        ) : (
          <Button loading={busy} onClick={onConnect}>
            Sign in to Instagram
          </Button>
        )}
      </div>
    </div>
  );
}
