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
  probe: () => void;
}

const NetContext = createContext<NetContextValue>({
  net: null,
  checking: false,
  offline: false,
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

  const probe = useCallback(() => {
    setChecking(true);
    netPing()
      .then(setNet)
      // A rejected command is itself evidence of no connectivity.
      .catch(() => setNet({ online: false, ms: null, host: null }))
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
    () => ({ net, checking, offline: net?.online === false, probe }),
    [net, checking, probe],
  );

  return <NetContext.Provider value={value}>{children}</NetContext.Provider>;
}
