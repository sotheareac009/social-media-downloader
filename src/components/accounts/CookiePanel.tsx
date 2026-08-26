import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/Button";
import { useToast } from "@/components/ui/Toast";
import { AlertIcon, CheckIcon, ShieldIcon, TrashIcon } from "@/components/ui/icons";
import {
  downloadMessage,
  sessionCheck,
  sessionClear,
  sessionImportCookies,
  sessionStatusFor,
  type CookiePlatform,
  type SessionStatus,
} from "@/lib/download";
import { toAuthError } from "@/lib/auth";

/**
 * Paste a cookie export instead of signing in through a window.
 *
 * WHY THIS EXISTS. An embedded login window cannot always finish the job: a
 * checkpoint, two-factor prompt or "suspicious device" wall stops it, and for
 * those accounts cookies obtained in a real browser are the only route. The
 * cookies are exactly what the downloader needs anyway — the login window's
 * whole purpose was to capture them.
 *
 * The paste stays here only long enough to be sent. Rust parses it, keeps this
 * platform's cookies and nothing else, and hands back the same non-secret
 * status the window produced — the frontend never sees a cookie value, and the
 * box is cleared the moment a save succeeds.
 */
export function CookiePanel({
  platform,
  label,
  connected: connectedProp,
  onSaved,
}: {
  platform: CookiePlatform;
  label: string;
  /**
   * Whether cookies are stored, when the page already tracks it. Omitted for
   * platforms it does not, in which case the panel asks for itself — a card
   * that needs the page to know about it is a card that cannot be reused.
   */
  connected?: boolean;
  onSaved?: (status: SessionStatus) => void;
}) {
  const toast = useToast();
  const [open, setOpen] = useState(false);
  const [text, setText] = useState("");
  const [saving, setSaving] = useState(false);
  const [checking, setChecking] = useState(false);
  const [clearing, setClearing] = useState(false);
  // Clearing throws away a working login, so it asks once. Not a modal — the
  // button becoming "Really clear?" is enough for something this recoverable.
  const [confirmClear, setConfirmClear] = useState(false);
  const [verdict, setVerdict] = useState<{ ok: boolean; message: string } | null>(null);
  const [status, setStatus] = useState<SessionStatus | null>(null);

  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  // Read once on mount. A failure here is not worth surfacing: the panel then
  // simply offers the paste box, which is the useful thing anyway.
  useEffect(() => {
    void sessionStatusFor(platform)
      .then((s) => mounted.current && setStatus(s))
      .catch(() => {});
  }, [platform]);

  const connected = connectedProp ?? status?.connected === true;

  /** Keep both the local view and the page (when it cares) in step. */
  const applyStatus = (next: SessionStatus) => {
    setStatus(next);
    onSaved?.(next);
  };

  const save = async () => {
    if (text.trim().length === 0) return;
    setSaving(true);
    setVerdict(null);
    try {
      const status = await sessionImportCookies(platform, text);
      // Cleared immediately: a session cookie left sitting in a textarea is
      // one screenshot away from being someone else's.
      setText("");
      setOpen(false);
      applyStatus(status);
      toast(
        "success",
        status.display_name
          ? `${label} cookies saved — signed in as ${status.display_name}.`
          : `${label} cookies saved.`,
      );
    } catch (e) {
      const err = toAuthError(e);
      // Shown in the panel rather than as a toast: the message says what to fix
      // about the paste, and it has to stay on screen while it is fixed.
      setVerdict({ ok: false, message: downloadMessage(err.code, err.message) });
    } finally {
      setSaving(false);
    }
  };

  const check = async () => {
    setChecking(true);
    setVerdict(null);
    try {
      const result = await sessionCheck(platform);
      const expiry =
        result.expires_at !== null
          ? ` Login cookies last until ${new Date(result.expires_at * 1000).toLocaleDateString()}.`
          : "";
      setVerdict({ ok: result.alive, message: result.message + expiry });
    } catch (e) {
      const err = toAuthError(e);
      setVerdict({ ok: false, message: downloadMessage(err.code, err.message) });
    } finally {
      setChecking(false);
    }
  };

  const clear = async () => {
    setClearing(true);
    setVerdict(null);
    try {
      const cleared = await sessionClear(platform);
      setConfirmClear(false);
      applyStatus(cleared);
      toast("info", `${label} cookies cleared.`);
    } catch (e) {
      const err = toAuthError(e);
      setVerdict({ ok: false, message: downloadMessage(err.code, err.message) });
    } finally {
      setClearing(false);
    }
  };

  return (
    <div className="cookiepanel">
      <div className="cookiepanel__head">
        <button
          className="btn btn--ghost btn--sm"
          type="button"
          onClick={() => setOpen((v) => !v)}
        >
          {open ? "Hide cookie box" : connected ? "Replace cookies" : "Paste cookies"}
        </button>
        {connected && (
          <>
            <button
              className="btn btn--ghost btn--sm"
              type="button"
              onClick={() => void check()}
              disabled={checking || clearing}
            >
              {checking ? "Checking…" : "Check if still valid"}
            </button>
            <button
              className={`btn btn--sm ${confirmClear ? "btn--danger" : "btn--ghost"}`}
              type="button"
              onClick={() => (confirmClear ? void clear() : setConfirmClear(true))}
              onBlur={() => setConfirmClear(false)}
              disabled={clearing}
            >
              <TrashIcon size={13} />
              {clearing
                ? "Clearing…"
                : confirmClear
                  ? "Really clear?"
                  : "Clear cookies"}
            </button>
          </>
        )}
      </div>

      {verdict && (
        <div className={`notice ${verdict.ok ? "notice--info" : "notice--danger"}`}>
          <span className="notice__icon">
            {verdict.ok ? <CheckIcon size={14} /> : <AlertIcon size={14} />}
          </span>
          <div>{verdict.message}</div>
        </div>
      )}

      {open && (
        <div className="cookiepanel__body">
          <label className="tg-field__label" htmlFor={`cookies-${platform}`}>
            {label} cookies
          </label>
          <textarea
            id={`cookies-${platform}`}
            className="input cookiepanel__input"
            rows={6}
            spellCheck={false}
            autoComplete="off"
            placeholder={`# Netscape HTTP Cookie File\n.${platform}.com\tTRUE\t/\tTRUE\t1819287955\t…`}
            value={text}
            disabled={saving}
            onChange={(e) => setText(e.target.value)}
          />
          <p className="cookiepanel__hint">
            Export them while signed in, with any "cookies.txt" browser
            extension, then paste the whole file here. Only {label} cookies are
            kept — anything else in the export is discarded.
          </p>
          <div className="cookiepanel__actions">
            <Button onClick={() => void save()} loading={saving} disabled={text.trim() === ""}>
              Save cookies
            </Button>
            <span className="cookiepanel__warn">
              <ShieldIcon size={13} />
              These cookies are your login. Treat the paste like a password.
            </span>
          </div>
        </div>
      )}
    </div>
  );
}
