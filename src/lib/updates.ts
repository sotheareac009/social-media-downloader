/**
 * In-app updates.
 *
 * The app fetches a small signed manifest, and installs the new bundle only if
 * its signature matches the public key compiled into the build. That check is
 * the whole security model: without it, anyone who could answer the update URL
 * could hand the app arbitrary code to run.
 *
 * A dev build has no published release to compare against, so every failure
 * here is treated as "nothing to update" rather than an error worth showing.
 */
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export interface UpdateInfo {
  version: string;
  /** Release notes from the manifest, when the release had a body. */
  notes: string | null;
  date: string | null;
}

/** The pending update, held so `install` uses the one that was announced. */
let pending: Update | null = null;

/**
 * Ask whether a newer version exists.
 *
 * Resolves to null when the app is current, when the endpoint cannot be
 * reached, or when this build has no updater configured at all — the caller
 * cannot act differently on any of those, and a startup toast about a
 * network blip helps nobody.
 */
export async function checkForUpdate(): Promise<UpdateInfo | null> {
  try {
    const update = await check();
    if (!update) {
      pending = null;
      return null;
    }
    pending = update;
    return {
      version: update.version,
      notes: update.body ?? null,
      date: update.date ?? null,
    };
  } catch {
    pending = null;
    return null;
  }
}

/**
 * Download and install the update found by the last check, then relaunch.
 *
 * `onProgress` receives 0-100. The total is only known once the server states
 * a length, so it stays null until then and the caller shows bytes instead.
 */
export async function installUpdate(
  onProgress?: (percent: number | null, downloaded: number) => void,
): Promise<void> {
  const update = pending;
  if (!update) throw new Error("No update has been found to install.");

  let total: number | null = null;
  let downloaded = 0;

  await update.downloadAndInstall((event) => {
    switch (event.event) {
      case "Started":
        total = event.data.contentLength ?? null;
        onProgress?.(total ? 0 : null, 0);
        break;
      case "Progress":
        downloaded += event.data.chunkLength;
        onProgress?.(total ? (downloaded / total) * 100 : null, downloaded);
        break;
      case "Finished":
        onProgress?.(100, downloaded);
        break;
    }
  });

  // Windows hands control to the installer, which closes the app itself; on
  // macOS and Linux the swap is done in place and the relaunch is ours to do.
  await relaunch();
}
