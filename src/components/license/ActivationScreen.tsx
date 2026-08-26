import { useCallback, useState, type FormEvent } from "react";
import {
  licenseActivate,
  licenseMessage,
  type LicenseStatus,
} from "@/lib/license";
import { toAuthError } from "@/lib/auth";
import { AlertIcon, CheckIcon, ShieldIcon } from "@/components/ui/icons";

/**
 * The gate shown when the app has no valid licence.
 *
 * A textarea rather than an input: an Ed25519 signature is 64 bytes, so keys
 * are long by construction and arrive by copy-paste. A single-line box would
 * hide most of what was pasted and makes a truncated paste hard to spot.
 */
export function ActivationScreen({
  onActivated,
}: {
  onActivated: (status: LicenseStatus) => void;
}) {
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = useCallback(
    async (e?: FormEvent) => {
      e?.preventDefault();
      if (busy || key.trim().length === 0) return;

      setBusy(true);
      setError(null);
      try {
        onActivated(await licenseActivate(key.trim()));
      } catch (err) {
        const parsed = toAuthError(err);
        setError(licenseMessage(parsed.code, parsed.message));
      } finally {
        setBusy(false);
      }
    },
    [busy, key, onActivated],
  );

  return (
    <div className="gate">
      <div className="gate__panel rise">
        <div className="gate__mark">
          <ShieldIcon size={20} />
        </div>

        <h1 className="gate__title">Activate SocialSync</h1>
        <p className="gate__lede">
          Enter the licence key from your purchase confirmation to unlock the
          app. It only needs to be entered once on this computer.
        </p>

        {error && (
          <div className="notice notice--danger gate__error">
            <span className="notice__icon">
              <AlertIcon size={14} />
            </span>
            <div>{error}</div>
          </div>
        )}

        <form onSubmit={submit}>
          <label className="gate__label" htmlFor="license-key">
            Licence key
          </label>
          <textarea
            id="license-key"
            className="gate__input"
            rows={3}
            spellCheck={false}
            autoComplete="off"
            autoFocus
            placeholder="SMD1-XXXXXX-XXXXXX-XXXXXX-…"
            value={key}
            disabled={busy}
            onChange={(e) => setKey(e.target.value)}
            onKeyDown={(e) => {
              // Enter submits; the key is one long token, so a newline in it is
              // always a paste artefact rather than something intended.
              if (e.key === "Enter" && !e.shiftKey) void submit();
            }}
          />

          <button
            className="btn btn--primary gate__submit"
            type="submit"
            disabled={busy || key.trim().length === 0}
            aria-busy={busy || undefined}
          >
            {busy ? <span className="btn__spinner" /> : <CheckIcon size={15} />}
            {busy ? "Checking…" : "Activate"}
          </button>
        </form>

        <p className="gate__hint">
          Lost your key? Reply to your purchase email and we'll resend it.
        </p>
      </div>
    </div>
  );
}
