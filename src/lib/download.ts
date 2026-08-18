/**
 * Typed bridge to the Rust download layer.
 *
 * Note the shape of what crosses this boundary: a job carries a title, a byte
 * count and a local path. The signed CDN URL the engine actually fetches from
 * stays in Rust, and no credential is involved on either side — public media
 * is fetched with no session at all.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Platforms this build will fetch public media from. */
export type Source = "facebook" | "tiktok";

export type JobStatus =
  | "queued"
  | "probing"
  | "downloading"
  | "completed"
  | "failed"
  | "cancelled";

export interface JobView {
  id: string;
  source: Source;
  url: string;
  status: JobStatus;
  title: string | null;
  uploader: string | null;
  duration_seconds: number | null;
  thumbnail_url: string | null;
  downloaded_bytes: number;
  total_bytes: number | null;
  speed_bps: number | null;
  eta_seconds: number | null;
  /** 0–1, only once a total is known. */
  fraction: number | null;
  output_path: string | null;
  error_code: string | null;
  error_message: string | null;
  created_at: number;
}

export interface MediaInfo {
  id: string;
  title: string;
  uploader: string | null;
  duration_seconds: number | null;
  thumbnail_url: string | null;
  estimated_bytes: number | null;
  extension: string | null;
}

/** Where files are saved, plus what the UI needs to offer a reset. */
export interface Destination {
  path: string;
  /** False when this is the built-in default. */
  is_custom: boolean;
  default_path: string;
}

export interface EngineStatus {
  available: boolean;
  path: string | null;
  version: string | null;
}

export interface ProgressEvent {
  id: string;
  downloaded_bytes: number;
  total_bytes: number | null;
  speed_bps: number | null;
  eta_seconds: number | null;
  fraction: number | null;
}

export const isTerminal = (s: JobStatus) =>
  s === "completed" || s === "failed" || s === "cancelled";

// ---------------------------------------------------------------- commands

export const downloadEngineStatus = () =>
  invoke<EngineStatus>("download_engine_status");

export const downloadInspect = (url: string) =>
  invoke<MediaInfo>("download_inspect", { url });

export const downloadStart = (url: string) =>
  invoke<JobView>("download_start", { url });

export const downloadList = () => invoke<JobView[]>("download_list");

export const downloadCancel = (id: string) =>
  invoke<JobView>("download_cancel", { id });

export const downloadRemove = (id: string) =>
  invoke<void>("download_remove", { id });

export const downloadClearFinished = () =>
  invoke<number>("download_clear_finished");

export const downloadGetDestination = () =>
  invoke<Destination>("download_get_destination");

export const downloadSetDestination = (path: string) =>
  invoke<Destination>("download_set_destination", { path });

export const downloadResetDestination = () =>
  invoke<Destination>("download_reset_destination");

/**
 * Open the OS folder picker and return the chosen path *without* applying it.
 *
 * Browsing and saving are separate on purpose — the result is a proposal the
 * user still has to confirm. Resolves to `null` when the picker is dismissed,
 * which is a normal outcome and must not be reported as a failure.
 */
export const downloadBrowseDestination = () =>
  invoke<string | null>("download_browse_destination");

export const downloadReveal = (path: string) =>
  invoke<void>("download_reveal", { path });

// ------------------------------------------------------------------ events

export interface DownloadEventHandlers {
  onCreated?: (job: JobView) => void;
  onUpdated?: (job: JobView) => void;
  onProgress?: (p: ProgressEvent) => void;
  onFinished?: (job: JobView) => void;
  onFailed?: (job: JobView) => void;
}

export async function subscribeToDownloadEvents(
  handlers: DownloadEventHandlers,
): Promise<UnlistenFn> {
  const unlisteners: UnlistenFn[] = [];
  const bind = async <T>(name: string, fn: ((p: T) => void) | undefined) => {
    if (fn) unlisteners.push(await listen<T>(name, (e) => fn(e.payload)));
  };

  await bind<JobView>("download://created", handlers.onCreated);
  await bind<JobView>("download://updated", handlers.onUpdated);
  await bind<ProgressEvent>("download://progress", handlers.onProgress);
  await bind<JobView>("download://finished", handlers.onFinished);
  await bind<JobView>("download://failed", handlers.onFailed);

  return () => unlisteners.forEach((u) => u());
}

// --------------------------------------------------------------- messaging

/**
 * Copy for a person. `media_not_public` is the one people will hit most, so it
 * says plainly that connecting an account would not help — because it wouldn't.
 */
export function downloadMessage(code: string | null, fallback: string): string {
  switch (code) {
    case "unsupported_url":
      return "That isn't a Facebook or TikTok video link.";
    case "engine_missing":
      return "The download engine (yt-dlp) isn't installed yet.";
    case "media_not_public":
      return "This post isn't public, so it can't be downloaded. Signing in wouldn't change that — this app only fetches posts anyone can already view.";
    case "no_media_found":
      return "No video was found at that link.";
    case "network":
      return "Couldn't reach the site. Check your connection.";
    case "download_path":
      return "The download folder can't be written to.";
    default:
      return fallback;
  }
}

// ------------------------------------------------------------------ format

export function formatBytes(bytes: number | null): string {
  if (bytes === null || bytes < 0) return "—";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}

export function formatSpeed(bps: number | null): string {
  if (!bps || bps <= 0) return "";
  return `${formatBytes(bps)}/s`;
}

export function formatDuration(seconds: number | null): string {
  if (seconds === null || seconds < 0) return "";
  const s = Math.round(seconds);
  const m = Math.floor(s / 60);
  const h = Math.floor(m / 60);
  const two = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${two(m % 60)}:${two(s % 60)}` : `${m}:${two(s % 60)}`;
}

export function formatEta(seconds: number | null): string {
  if (seconds === null || seconds <= 0) return "";
  if (seconds < 60) return `${Math.round(seconds)}s left`;
  const m = Math.round(seconds / 60);
  return m < 60 ? `${m} min left` : `${Math.round(m / 60)} h left`;
}
