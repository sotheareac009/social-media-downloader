import { useCallback, useEffect, useRef, useState } from "react";
import {
  telegramClearConfig,
  telegramGetConfig,
  telegramSetConfig,
  type TelegramConfig,
} from "@/lib/telegram";
import { Button } from "@/components/ui/Button";
import { useToast } from "@/components/ui/Toast";
import { CheckIcon, SlidersIcon } from "@/components/ui/icons";

export function SettingsPage() {
  const toast = useToast();
  const [config, setConfig] = useState<TelegramConfig | null>(null);
  const [apiId, setApiId] = useState("");
  const [apiHash, setApiHash] = useState("");
  const [busy, setBusy] = useState(false);

  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  useEffect(() => {
    void (async () => {
      const cfg = await telegramGetConfig().catch(() => null);
      if (!mounted.current || !cfg) return;
      setConfig(cfg);
      if (cfg.configured) {
        setApiId(String(cfg.api_id));
        setApiHash(cfg.api_hash);
      }
    })();
  }, []);

  const save = useCallback(async () => {
    setBusy(true);
    try {
      const next = await telegramSetConfig(apiId, apiHash);
      setConfig(next);
      toast("success", "Telegram credentials saved. Open the Telegram page to sign in.");
    } catch (e) {
      const msg =
        typeof e === "object" && e && "message" in e
          ? String((e as { message: unknown }).message)
          : "Couldn't save. Check the values.";
      toast("error", msg);
    } finally {
      if (mounted.current) setBusy(false);
    }
  }, [apiId, apiHash, toast]);

  const clear = useCallback(async () => {
    setBusy(true);
    try {
      const next = await telegramClearConfig();
      setConfig(next);
      if (!next.configured) {
        setApiId("");
        setApiHash("");
      }
      toast("info", "Saved credentials cleared.");
    } catch {
      toast("error", "Couldn't clear.");
    } finally {
      if (mounted.current) setBusy(false);
    }
  }, [toast]);

  const dirty =
    apiId.trim() !== (config?.configured ? String(config.api_id) : "") ||
    apiHash.trim() !== (config?.configured ? config.api_hash : "");

  return (
    <div className="page">
      <header className="page__header rise">
        <span className="page__eyebrow">
          <SlidersIcon size={12} />
          Settings
        </span>
        <h1 className="page__title">Settings</h1>
        <p className="page__lede">
          Configure services here. These are saved on this computer, so a
          packaged build works without editing <code>.env</code>.
        </p>
      </header>

      <section className="settings-card rise">
        <div className="settings-card__head">
          <h2 className="settings-card__title">Telegram</h2>
          {config?.configured && (
            <span className="badge badge--success">
              <CheckIcon size={11} /> Configured
            </span>
          )}
        </div>
        <p className="settings-card__lede">
          From{" "}
          <code>https://my.telegram.org</code> → API development tools. The
          api_id is the short number; the api_hash is the long hex string.
        </p>

        <label className="tg-field__label" htmlFor="api-id">API ID</label>
        <input
          id="api-id"
          className="tg-field__input"
          inputMode="numeric"
          placeholder="38837128"
          value={apiId}
          disabled={busy}
          onChange={(e) => setApiId(e.target.value)}
        />

        <label className="tg-field__label" htmlFor="api-hash" style={{ marginTop: 14 }}>
          API hash
        </label>
        <input
          id="api-hash"
          className="tg-field__input"
          placeholder="5bbe60bb99e8319216ffb68f745d7283"
          value={apiHash}
          disabled={busy}
          onChange={(e) => setApiHash(e.target.value)}
          spellCheck={false}
        />

        <div className="settings-card__actions">
          {config?.configured && (
            <Button variant="ghost" onClick={() => void clear()} disabled={busy}>
              Clear
            </Button>
          )}
          <Button
            onClick={() => void save()}
            loading={busy}
            disabled={!apiId.trim() || !apiHash.trim() || !dirty}
          >
            Save
          </Button>
        </div>
      </section>
    </div>
  );
}
