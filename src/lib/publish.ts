/**
 * Typed bridge to the Rust publishing layer.
 *
 * An "account" here is an Android app on an emulator that the user already
 * signed into by hand. There is no password, token or cookie in any of these
 * types, because the app never has one: the session stays on the device.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Platform = "facebook" | "instagram" | "tiktok" | "youtube";

export type JobStatus =
  | "pending"
  | "uploading"
  | "publishing"
  | "published"
  /** Stopped on purpose: the app is open and waiting for the person. */
  | "needs_attention"
  | "failed"
  | "cancelled";

export type AccountStatus =
  | "connected"
  | "app_missing"
  | "device_offline"
  | "device_missing";

export interface Account {
  id: string;
  name: string;
  platform: Platform;
  ldplayer_instance_id: string;
  package_name: string;
  created_at: number;
}

export interface AccountView extends Account {
  status: AccountStatus;
  device_name: string | null;
  device_online: boolean;
  detail: string | null;
}

/** How several selected assets become posts. */
export type PostMode = "album" | "single";

export interface PublishJob {
  id: string;
  media_id: string;
  account_id: string;
  caption: string;
  status: JobStatus;
  /** 0–1, coarse: the meaningful units are steps, not bytes. */
  progress: number;
  step: string | null;
  error_code: string | null;
  error: string | null;
  screenshot_path: string | null;
  created_at: number;
  started_at: number | null;
  completed_at: number | null;
  account_name: string;
  platform: Platform;
  device_id: string;
  /** First asset's name — what a compact row shows. */
  media_name: string;
  /** Every asset, in carousel order. */
  media_names: string[];
  /** >1 means this job is an album post. */
  media_count: number;
}

export interface QueueSummary {
  pending: number;
  active: number;
  published: number;
  needs_attention: number;
  failed: number;
}

export interface DiscoveredApp {
  platform: Platform;
  package: string;
  label: string;
}

export interface PlatformInfo {
  id: Platform;
  label: string;
  packages: string[];
}

export const publishPlatforms = () => invoke<PlatformInfo[]>("publish_platforms");

export const publishAccounts = () => invoke<AccountView[]>("publish_accounts");

export const publishDiscoverAccounts = (deviceId: string) =>
  invoke<DiscoveredApp[]>("publish_discover_accounts", { deviceId });

export const publishAddAccount = (args: {
  name: string;
  platform: Platform;
  deviceId: string;
  package: string;
}) => invoke<Account>("publish_add_account", args);

export const publishRenameAccount = (id: string, name: string) =>
  invoke<void>("publish_rename_account", { id, name });

export const publishRemoveAccount = (id: string) =>
  invoke<void>("publish_remove_account", { id });

/**
 * Queue the selected media to every selected account. Returns before any work
 * starts.
 *
 * `mode` is required, not defaulted: guessing wrong publishes three posts where
 * one album was wanted, and that is not undoable.
 */
export const publishSubmit = (args: {
  paths: string[];
  caption: string;
  accountIds: string[];
  mode: PostMode;
}) => invoke<PublishJob[]>("publish_submit", args);

export const publishJobs = () => invoke<PublishJob[]>("publish_jobs");

export const publishSummary = () => invoke<QueueSummary>("publish_summary");

export const publishRetry = (id: string) => invoke<PublishJob>("publish_retry", { id });

export const publishCancel = (id: string) => invoke<PublishJob>("publish_cancel", { id });

export const publishRemoveJob = (id: string) => invoke<void>("publish_remove_job", { id });

export const publishClearFinished = () => invoke<number>("publish_clear_finished");

export function subscribeToPublishEvents(handlers: {
  onCreated?: (job: PublishJob) => void;
  onUpdated?: (job: PublishJob) => void;
  onFinished?: (job: PublishJob) => void;
}): Promise<UnlistenFn> {
  const pending: Promise<UnlistenFn>[] = [];
  if (handlers.onCreated)
    pending.push(listen<PublishJob>("publish://created", (e) => handlers.onCreated!(e.payload)));
  if (handlers.onUpdated)
    pending.push(listen<PublishJob>("publish://updated", (e) => handlers.onUpdated!(e.payload)));
  if (handlers.onFinished)
    pending.push(listen<PublishJob>("publish://finished", (e) => handlers.onFinished!(e.payload)));

  return Promise.all(pending).then((unlisteners) => () => {
    for (const un of unlisteners) un();
  });
}

/**
 * One place that decides how a status reads, so the badge, the dot and the
 * summary can never describe the same job differently.
 */
export const JOB_STATUS_LABEL: Record<JobStatus, string> = {
  pending: "Queued",
  uploading: "Copying video",
  publishing: "Publishing",
  published: "Published",
  needs_attention: "Needs you",
  failed: "Failed",
  cancelled: "Cancelled",
};

export const JOB_STATUS_TONE: Record<JobStatus, "success" | "warning" | "danger" | "muted" | "active"> = {
  pending: "muted",
  uploading: "active",
  publishing: "active",
  published: "success",
  needs_attention: "warning",
  failed: "danger",
  cancelled: "muted",
};

export const ACCOUNT_STATUS_LABEL: Record<AccountStatus, string> = {
  connected: "Connected",
  app_missing: "App not installed",
  device_offline: "Emulator stopped",
  device_missing: "Emulator missing",
};

export const isJobActive = (status: JobStatus) =>
  status === "pending" || status === "uploading" || status === "publishing";
