import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { downloadEngineStatus, type EngineStatus } from "@/lib/download";

const UNAVAILABLE: EngineStatus = {
  available: false,
  path: null,
  version: null,
  has_ffmpeg: false,
  ffmpeg_path: null,
  has_lister: false,
  lister_version: null,
};

interface EngineContextValue {
  /** Null only until the first probe resolves. */
  engine: EngineStatus | null;
  ready: boolean;
  /**
   * True only once a probe has actually reported the engine absent.
   *
   * Deliberately not `!ready`: while the first probe is in flight the answer
   * is unknown, and treating unknown as missing disables the paste box at
   * launch with no panel on screen to explain why. Gate on this, not `ready`.
   */
  missing: boolean;
  rechecking: boolean;
  /** Re-probe on demand. Resolves with the fresh status so callers can report. */
  recheck: () => Promise<EngineStatus>;
}

const EngineContext = createContext<EngineContextValue>({
  engine: null,
  ready: false,
  missing: false,
  rechecking: false,
  recheck: async () => UNAVAILABLE,
});

export const useEngineStatus = () => useContext(EngineContext);

/**
 * Probes for yt-dlp/ffmpeg/gallery-dl **once per app launch**.
 *
 * Detection is not free: it walks the filesystem and spawns `yt-dlp --version`.
 * Previously the Home and Downloads pages each ran it on mount, and because
 * Downloads unmounts when you navigate away, every visit paid for the probe
 * again - so a user with the tools correctly installed still watched it
 * re-check on every trip to the page.
 *
 * The result is cached here for the session. It changes only when tools are
 * installed or removed, which are explicit events: the `tools-ready` signal
 * from first-launch setup, and the manual "Re-check" button.
 */
export function EngineStatusProvider({ children }: { children: ReactNode }) {
  const [engine, setEngine] = useState<EngineStatus | null>(null);
  const [rechecking, setRechecking] = useState(false);

  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const probe = useCallback(async (): Promise<EngineStatus> => {
    try {
      const status = await downloadEngineStatus();
      if (mounted.current) setEngine(status);
      return status;
    } catch {
      // A failed probe means we could not determine the engine exists, which
      // for every UI purpose is the same as it not being there.
      if (mounted.current) setEngine(UNAVAILABLE);
      return UNAVAILABLE;
    }
  }, []);

  const recheck = useCallback(async () => {
    setRechecking(true);
    try {
      return await probe();
    } finally {
      if (mounted.current) setRechecking(false);
    }
  }, [probe]);

  // The one probe for the whole session.
  useEffect(() => {
    void probe();
  }, [probe]);

  // First-launch setup can install the tools after this provider has already
  // concluded they were missing, so it announces itself when done.
  useEffect(() => {
    const onReady = () => void probe();
    window.addEventListener("tools-ready", onReady);
    return () => window.removeEventListener("tools-ready", onReady);
  }, [probe]);

  const value = useMemo<EngineContextValue>(
    () => ({
      engine,
      ready: engine?.available === true,
      missing: engine !== null && engine.available === false,
      rechecking,
      recheck,
    }),
    [engine, rechecking, recheck],
  );

  return <EngineContext.Provider value={value}>{children}</EngineContext.Provider>;
}
