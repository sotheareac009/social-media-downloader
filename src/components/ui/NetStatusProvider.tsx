import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { netPing, type NetStatus } from "@/lib/net";

interface NetContextValue {
  /** Null until the first probe resolves. */
  net: NetStatus | null;
  checking: boolean;
  /**
   * True only once a probe has actually failed.
   *
   * Deliberately not `!online`: during the first probe the answer is unknown,
   * and treating unknown as offline would block downloads for a moment on
   * every launch.
   */
  offline: boolean;
  /**
   * Why the last probe failed, when it failed for a reason other than "no
   * network" - e.g. the command is missing or panicked.
   *
   * A rejected invoke used to be turned straight into `online: false`, which
   * reported a broken check as "you are offline" and sent the user to look at
   * their wifi. Keep the reason so the UI can say what actually happened.
   */
  error: string | null;
  probe: () => void;
}

const NetContext = createContext<NetContextValue>({
  net: null,
  checking: false,
  offline: false,
  error: null,
  probe: () => {},
});

export const useNetStatus = () => useContext(NetContext);

const POLL_MS = 15_000;

/**
 * Single source of truth for connectivity.
 *
 * One provider rather than a hook per consumer: the sidebar indicator and the
 * Downloads page must agree, and two independent pollers would both ping every
 * 15s and could disagree for a whole interval.
 */
export function NetStatusProvider({ children }: { children: ReactNode }) {
  const [net, setNet] = useState<NetStatus | null>(null);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const probe = useCallback(() => {
    setChecking(true);
    netPing()
      .then((s) => {
        setNet(s);
        setError(null);
      })
      .catch((e: unknown) => {
        // Distinguish "the probe ran and found no route" from "the probe could
        // not run at all". Both leave the app unusable for downloads, but only
        // the first is the user's network.
        setNet({ online: false, ms: null, host: null });
        setError(
          e instanceof Error && e.message
            ? e.message
            : typeof e === "string" && e
              ? e
              : "the connectivity check could not run",
        );
      })
      .finally(() => setChecking(false));
  }, []);

  useEffect(() => {
    probe();
    const id = window.setInterval(probe, POLL_MS);
    // The OS tells us immediately; the timer is the backstop for cases it
    // misses, such as a router that is up but has no route to the internet.
    const onChange = () => probe();
    window.addEventListener("online", onChange);
    window.addEventListener("offline", onChange);
    return () => {
      window.clearInterval(id);
      window.removeEventListener("online", onChange);
      window.removeEventListener("offline", onChange);
    };
  }, [probe]);

  const value = useMemo<NetContextValue>(
    () => ({ net, checking, offline: net?.online === false, error, probe }),
    [net, checking, error, probe],
  );

  return <NetContext.Provider value={value}>{children}</NetContext.Provider>;
}
