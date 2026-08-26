import { useCallback, useEffect, useRef, useState } from "react";
import { useToast } from "@/components/ui/Toast";
import { DownloadIcon } from "@/components/ui/icons";
import { getVersion } from "@tauri-apps/api/app";
import { checkForUpdate, installUpdate, type UpdateInfo } from "@/lib/updates";

type State = "idle" | "checking" | "available" | "installing";

/**
 * The version number in the sidebar, doubling as the update control.
 *
 * It is the version label until there is something to say, at which point it
 * becomes the button that says it. A separate "check for updates" screen would
 * be a page nobody visits, and a nag dialog on launch interrupts work to
 * announce something that can wait.
 *
 * The check on mount is deliberately silent: an unreachable endpoint, a dev
 * build with no release, or a network blip must not produce a toast at every
 * launch. Only a check the user asked for reports that it found nothing.
 */
export function UpdateBadge() {
  const toast = useToast();
  // Read from the built app rather than passed in: a hardcoded string drifts
  // from the real version the moment someone bumps one and not the other, and
  // it is the number people quote in bug reports.
  const [version, setVersion] = useState("");
  const [state, setState] = useState<State>("idle");
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [percent, setPercent] = useState<number | null>(null);

  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const runCheck = useCallback(
    async (announce: boolean) => {
      setState("checking");
      const found = await checkForUpdate();
      if (!mounted.current) return;
      setUpdate(found);
      setState(found ? "available" : "idle");
      if (announce && !found) {
        toast("info", `You're on the latest version (${version}).`);
      }
    },
    [toast, version],
  );

  useEffect(() => {
    void getVersion()
      .then((v) => mounted.current && setVersion(`v${v}`))
      .catch(() => {});
  }, []);

  useEffect(() => {
    void runCheck(false);
    // Once per launch. Polling would only matter to someone who leaves the app
    // open for days, and they can press the button.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const install = useCallback(async () => {
    setState("installing");
    setPercent(0);
    try {
      await installUpdate((pct) => {
        if (mounted.current) setPercent(pct);
      });
      // Reached only if the relaunch did not happen — otherwise the process is
      // already gone.
    } catch (e) {
      if (!mounted.current) return;
      setState("available");
      setPercent(null);
      toast(
        "error",
        `The update couldn't be installed: ${e instanceof Error ? e.message : String(e)}`,
      );
    }
  }, [toast]);

  if (state === "installing") {
    return (
      <span className="sidebar__phase sidebar__phase--busy">
        {percent === null ? "Downloading…" : `Updating ${Math.round(percent)}%`}
      </span>
    );
  }

  if (state === "available" && update) {
    return (
      <button
        className="updatebtn"
        type="button"
        onClick={() => void install()}
        title={update.notes ?? `Install version ${update.version} and restart`}
      >
        <DownloadIcon size={12} />
        Update to {update.version}
      </button>
    );
  }

  return (
    <button
      className="sidebar__phase sidebar__phase--button"
      type="button"
      onClick={() => void runCheck(true)}
      disabled={state === "checking"}
      title="Check for updates"
    >
      {state === "checking" ? "Checking…" : version || "…"}
    </button>
  );
}
