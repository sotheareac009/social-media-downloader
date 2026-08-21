import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface ToolsStatus {
  ytdlp: boolean;
  ffmpeg: boolean;
  /** Nothing left to install for the core download path. */
  ready: boolean;
  /** True where auto-install is supported (macOS). */
  can_install: boolean;
}

export type ToolState = "downloading" | "installed" | "skipped" | "failed";

export interface ToolsProgress {
  tool: string;
  state: ToolState;
  step: number;
  total: number;
  error: string | null;
  /** Bytes received so far for the download in flight. */
  downloaded_bytes: number;
  /** Absent when the server sends no Content-Length. */
  total_bytes: number | null;
  /** Recent throughput, measured over the last reporting window. */
  bytes_per_sec: number | null;
}

/** "163 MB", "17.4 MB", "812 KB" — sized so the unit stays readable. */
export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${Math.round(n / 1024)} KB`;
  const mb = n / (1024 * 1024);
  if (mb < 1024) return `${mb < 10 ? mb.toFixed(1) : Math.round(mb)} MB`;
  return `${(mb / 1024).toFixed(1)} GB`;
}

/**
 * Rough time remaining, in words.
 *
 * Deliberately coarse: a byte-accurate countdown on a fluctuating connection
 * jitters and reads as broken. "about 2 min left" is more honest than a
 * precise number that changes every tick.
 */
export function formatEta(
  downloaded: number,
  total: number | null,
  bytesPerSec: number | null,
): string | null {
  if (!total || !bytesPerSec || bytesPerSec <= 0) return null;
  const remaining = total - downloaded;
  if (remaining <= 0) return null;
  const secs = remaining / bytesPerSec;
  if (secs < 10) return "almost done";
  if (secs < 60) return `about ${Math.ceil(secs / 5) * 5}s left`;
  const mins = Math.ceil(secs / 60);
  return `about ${mins} min left`;
}

export const toolsStatus = () => invoke<ToolsStatus>("tools_status");
export const toolsInstall = () => invoke<ToolsStatus>("tools_install");

export const onToolsProgress = (
  cb: (p: ToolsProgress) => void,
): Promise<UnlistenFn> =>
  listen<ToolsProgress>("tools://progress", (e) => cb(e.payload));
