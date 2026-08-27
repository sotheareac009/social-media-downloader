import { useCallback, useEffect, useRef, useState } from "react";
import {
  TelegramLogin,
  TelegramLoginError,
  telegramClearSession,
  telegramGetConfig,
  telegramStatus,
  telegramValidateSession,
  type TelegramConfig,
  type TelegramStatus,
} from "@/lib/telegram";
import { Button } from "@/components/ui/Button";
import { MessageLookup } from "@/components/telegram/MessageLookup";
import { useToast } from "@/components/ui/Toast";
import { AlertIcon, ArrowLeftIcon, CheckIcon, ShieldIcon, TelegramIcon, XIcon } from "@/components/ui/icons";

type Step = "phone" | "code" | "password";

export function TelegramPage({ onBack }: { onBack?: () => void }) {
  const toast = useToast();
  const [config, setConfig] = useState<TelegramConfig | null>(null);
  const [status, setStatus] = useState<TelegramStatus | null>(null);
  const [checking, setChecking] = useState(true);

  const [step, setStep] = useState<Step>("phone");
  const [phone, setPhone] = useState("");
  const [code, setCode] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // The live login attempt is kept in a ref: it holds a socket, not render state.
  const login = useRef<TelegramLogin | null>(null);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      void login.current?.cancel();
    };
  }, []);

  useEffect(() => {
    void (async () => {
      try {
        const cfg = await telegramGetConfig();
        if (!mounted.current) return;
        setConfig(cfg);
        const st = await telegramStatus();
        if (!mounted.current) return;
        setStatus(st);
        // A stored session can expire; confirm it before claiming connected.
        if (cfg.configured && st.connected) {
          const ok = await telegramValidateSession(cfg).catch(() => false);
          if (mounted.current && !ok) {
            setStatus({ connected: false, connected_at: null, display_name: null });
          }
        }
      } finally {
        if (mounted.current) setChecking(false);
      }
    })();
  }, []);

  const reset = useCallback(() => {
    void login.current?.cancel();
    login.current = null;
    setStep("phone");
    setCode("");
    setPassword("");
    setError(null);
  }, []);

  const sendCode = useCallback(async () => {
    if (!config) return;
    setBusy(true);
    setError(null);
    try {
      const attempt = new TelegramLogin(config);
      await attempt.start(phone);
      login.current = attempt;
      setStep("code");
    } catch (e) {
      setError(e instanceof TelegramLoginError ? e.message : "Couldn't start the login.");
    } finally {
      if (mounted.current) setBusy(false);
    }
  }, [config, phone]);

  const submitCode = useCallback(async () => {
    if (!login.current) return;
    setBusy(true);
    setError(null);
    try {
      const next = await login.current.submitCode(code);
      if (next === "password") {
        setStep("password");
      } else {
        setStatus(await telegramStatus());
        reset();
        toast("success", "Telegram connected.");
      }
    } catch (e) {
      setError(e instanceof TelegramLoginError ? e.message : "That code wasn't accepted.");
    } finally {
      if (mounted.current) setBusy(false);
    }
  }, [code, reset, toast]);

  const submitPassword = useCallback(async () => {
    if (!login.current) return;
    setBusy(true);
    setError(null);
    try {
      await login.current.submitPassword(password);
      setStatus(await telegramStatus());
      reset();
      toast("success", "Telegram connected.");
    } catch (e) {
      setError(e instanceof TelegramLoginError ? e.message : "That password wasn't accepted.");
    } finally {
      if (mounted.current) setBusy(false);
    }
  }, [password, reset, toast]);

  const disconnect = useCallback(async () => {
    setBusy(true);
    try {
      setStatus(await telegramClearSession());
      reset();
      toast("info", "Telegram disconnected.");
    } catch {
      toast("error", "Couldn't disconnect.");
    } finally {
      if (mounted.current) setBusy(false);
    }
  }, [reset, toast]);

  return (
    <div className="page">
      <header className="page__header rise">
        {onBack && (
          <button type="button" className="page__back" onClick={onBack}>
            <ArrowLeftIcon size={14} /> Accounts
          </button>
        )}
        <span className="page__eyebrow page__eyebrow--telegram">
          <TelegramIcon size={12} />
          Telegram
        </span>
        <h1 className="page__title">Connect Telegram</h1>
        <p className="page__lede">
          Sign in with your phone number to link your Telegram account. The code
          is sent to your Telegram app; your number, code and 2FA password are
          never stored.
        </p>
      </header>

      {checking ? (
        <div className="tg-card"><CardSkeleton /></div>
      ) : !config?.configured ? (
        <NotConfigured />
      ) : status?.connected ? (
        <Connected status={status} busy={busy} onDisconnect={() => void disconnect()} />
      ) : (
        <div className="tg-card rise">
          <Stepper step={step} />
          {error && (
            <div className="notice notice--danger" style={{ marginBottom: 14 }}>
              <span className="notice__icon"><AlertIcon size={14} /></span>
              <div>{error}</div>
            </div>
          )}

          {step === "phone" && (
            <Field
              label="Phone number"
              hint="Include your country code, e.g. +855…"
              value={phone}
              onChange={setPhone}
              placeholder="+855 12 345 678"
              type="tel"
              busy={busy}
              cta="Send code"
              onSubmit={() => void sendCode()}
            />
          )}

          {step === "code" && (
            <Field
              label="Login code"
              hint="Telegram sent a code to your other devices."
              value={code}
              onChange={setCode}
              placeholder="12345"
              type="text"
              busy={busy}
              cta="Verify"
              onSubmit={() => void submitCode()}
              onBack={reset}
            />
          )}

          {step === "password" && (
            <Field
              label="Two-step password"
              hint="Your account has 2FA enabled — enter your cloud password."
              value={password}
              onChange={setPassword}
              placeholder="Your 2FA password"
              type="password"
              busy={busy}
              cta="Sign in"
              onSubmit={() => void submitPassword()}
              onBack={reset}
            />
          )}
        </div>
      )}

      <Assurance />
    </div>
  );
}

function Stepper({ step }: { step: Step }) {
  const order: Step[] = ["phone", "code", "password"];
  const labels: Record<Step, string> = {
    phone: "Phone",
    code: "Code",
    password: "2FA",
  };
  const active = order.indexOf(step);
  return (
    <div className="tg-steps">
      {order.map((s, i) => (
        <div
          key={s}
          className={`tg-step ${i === active ? "tg-step--active" : ""} ${
            i < active ? "tg-step--done" : ""
          }`.trim()}
        >
          <span className="tg-step__dot">{i < active ? <CheckIcon size={11} /> : i + 1}</span>
          {labels[s]}
        </div>
      ))}
    </div>
  );
}

function Field({
  label,
  hint,
  value,
  onChange,
  placeholder,
  type,
  busy,
  cta,
  onSubmit,
  onBack,
}: {
  label: string;
  hint: string;
  value: string;
  onChange: (v: string) => void;
  placeholder: string;
  type: string;
  busy: boolean;
  cta: string;
  onSubmit: () => void;
  onBack?: () => void;
}) {
  return (
    <form
      className="tg-field"
      onSubmit={(e) => {
        e.preventDefault();
        if (!busy && value.trim()) onSubmit();
      }}
    >
      <label className="tg-field__label">{label}</label>
      <input
        className="tg-field__input"
        type={type}
        value={value}
        placeholder={placeholder}
        autoComplete="off"
        autoFocus
        disabled={busy}
        onChange={(e) => onChange(e.target.value)}
      />
      <p className="tg-field__hint">{hint}</p>
      <div className="tg-field__actions">
        {onBack && (
          <Button variant="ghost" type="button" onClick={onBack} disabled={busy}>
            Start over
          </Button>
        )}
        <Button type="submit" loading={busy} disabled={!value.trim()}>
          {cta}
        </Button>
      </div>
    </form>
  );
}

function Connected({
  status,
  busy,
  onDisconnect,
}: {
  status: TelegramStatus;
  busy: boolean;
  onDisconnect: () => void;
}) {
  return (
    <div className="tg-card tg-card--ok rise">
      <div className="tg-connected">
        <span className="tg-connected__icon tg-connected__icon--brand"><TelegramIcon size={18} /></span>
        <div>
          <div className="tg-connected__title">Telegram is connected</div>
          <div className="tg-connected__sub">
            {status.connected_at
              ? `Signed in ${new Date(status.connected_at * 1000).toLocaleDateString()}`
              : "Signed in"}
          </div>
        </div>
        <Button variant="danger" onClick={onDisconnect} loading={busy} icon={<XIcon size={14} />}>
          Disconnect
        </Button>
      </div>

      {/* Only useful once signed in: opening a message needs the session. */}
      <MessageLookup />
    </div>
  );
}

function NotConfigured() {
  return (
    <div className="tg-card rise">
      <div className="engine engine--missing" style={{ border: 0, background: "transparent", padding: 0 }}>
        <span className="engine__icon"><AlertIcon size={14} /></span>
        <div className="engine__body">
          <div className="engine__title">Telegram isn't configured yet</div>
          <p className="engine__lede">
            Logging in needs an application <strong>api_id</strong> and{" "}
            <strong>api_hash</strong>. Create one for yourself, once:
          </p>
          <ol className="tg-setup">
            <li>Open <code>https://my.telegram.org</code> and sign in.</li>
            <li>Go to <strong>API development tools</strong> and create an app.</li>
            <li>
              Put the values in your <code>.env</code>:
              <pre className="tg-env">TELEGRAM_API_ID=1234567{"\n"}TELEGRAM_API_HASH=abc123…</pre>
            </li>
            <li>Restart the app.</li>
          </ol>
        </div>
      </div>
    </div>
  );
}

function CardSkeleton() {
  return (
    <div>
      <div className="skeleton" style={{ width: 180, height: 14 }} />
      <div className="skeleton" style={{ width: "100%", height: 40, marginTop: 14, borderRadius: 8 }} />
    </div>
  );
}

function Assurance() {
  return (
    <section className="assurance rise" style={{ animationDelay: "120ms" }}>
      <div className="assurance__title">
        <ShieldIcon size={14} />
        How your Telegram login is handled
      </div>
      <ul className="assurance__list">
        {[
          "The login talks to Telegram directly over an encrypted MTProto connection.",
          "Your phone number, the code and any 2FA password are used to sign in and never stored.",
          "Only the resulting session is kept, in an owner-only file — never in a database or this window's storage.",
          "Disconnecting deletes that session from this computer.",
        ].map((line) => (
          <li key={line}>
            <CheckIcon size={13} className="assurance__tick" />
            <span>{line}</span>
          </li>
        ))}
      </ul>
    </section>
  );
}
