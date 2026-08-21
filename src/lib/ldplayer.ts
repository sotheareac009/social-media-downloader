/**
 * Typed bridge to the Rust device layer (LDPlayer + ADB).
 *
 * Note what crosses this boundary: instance names, a serial, a file path, a
 * screenshot. No social-media credential appears here because none exists —
 * the login lives inside the Android app on the emulator, and this app never
 * reads it. See the module note on `ldplayer` in Rust for why that boundary is
 * structural rather than a promise.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** How the app came to know about a device. */
export type DeviceKind = "ldplayer" | "adb";

export type DeviceState = "stopped" | "booting" | "unreachable" | "online";

export interface DeviceView {
  /** `ld:0` or `adb:emulator-5554`. Stable across reboots. */
  id: string;
  kind: DeviceKind;
  index: number | null;
  name: string;
  state: DeviceState;
  serial: string | null;
  model: string | null;
  android_release: string | null;
  packages: string[] | null;
  error: string | null;
}

export interface DeviceEnvironment {
  adb_available: boolean;
  adb_path: string | null;
  adb_version: string | null;
  ldplayer_available: boolean;
  ldplayer_path: string | null;
  /** False off Windows, where LDPlayer does not exist at all. */
  ldplayer_supported: boolean;
  remote_dir: string;
  max_concurrent: number;
  verbose_logging: boolean;
  cleanup_after_publish: boolean;
  /** Folders detection looked in. Only populated when LDPlayer wasn't found. */
  searched: string[];
}

export interface DeviceSettings {
  ldplayer_path: string | null;
  adb_path: string | null;
  remote_dir: string;
  remote_image_dir: string;
  max_concurrent: number;
  /** Tap the app's own Post button once the composer is open. */
  auto_post: boolean;
  verbose_logging: boolean;
  cleanup_after_publish: boolean;
}

export interface LogLine {
  at: number;
  level: "info" | "warn" | "error";
  scope: string | null;
  message: string;
}

export const ldplayerEnvironment = () =>
  invoke<DeviceEnvironment>("ldplayer_environment");

export const ldplayerRedetect = () =>
  invoke<DeviceEnvironment>("ldplayer_redetect");

export const ldplayerGetSettings = () =>
  invoke<DeviceSettings>("ldplayer_get_settings");

export const ldplayerSetSettings = (settings: DeviceSettings) =>
  invoke<DeviceSettings>("ldplayer_set_settings", { settings });

export const ldplayerListDevices = () =>
  invoke<DeviceView[]>("ldplayer_list_devices");

export const ldplayerStart = (deviceId: string) =>
  invoke<DeviceView>("ldplayer_start", { deviceId });

export const ldplayerStop = (deviceId: string) =>
  invoke<DeviceView>("ldplayer_stop", { deviceId });

/** Boot if needed and wait for Android. Can take minutes on a cold instance. */
export const ldplayerConnect = (deviceId: string) =>
  invoke<DeviceView>("ldplayer_connect", { deviceId });

/**
 * Attach to a device by address — `5555`, `127.0.0.1:5555`, or `host:port`.
 * The escape hatch for anything auto-discovery misses.
 */
export const ldplayerConnectEndpoint = (address: string) =>
  invoke<DeviceView>("ldplayer_connect_endpoint", { address });

export const ldplayerPackages = (deviceId: string) =>
  invoke<string[]>("ldplayer_packages", { deviceId });

/** Whether Android will index a file as video or as an image. */
export type MediaCollection = "video" | "image";

export interface TransferredMedia {
  remote_path: string;
  /** `content://media/...` — how the file is handed to another app. */
  content_uri: string | null;
  collection: MediaCollection;
}

/** Copy media to a device and make the gallery see it. */
export const ldplayerTransferMedia = (deviceId: string, path: string) =>
  invoke<TransferredMedia>("ldplayer_transfer_media", { deviceId, path });

export const ldplayerLaunchApp = (deviceId: string, packageName: string) =>
  invoke<void>("ldplayer_launch_app", { deviceId, package: packageName });

export const ldplayerStopApp = (deviceId: string, packageName: string) =>
  invoke<void>("ldplayer_stop_app", { deviceId, package: packageName });

export const ldplayerScreenshot = (deviceId: string, label?: string) =>
  invoke<string>("ldplayer_screenshot", { deviceId, label: label ?? null });

/** Multi-select. Empty when the user cancelled the picker — not an error. */
export const ldplayerPickMedia = () => invoke<string[]>("ldplayer_pick_media");

/** Extensions the picker offers, and which collection each lands in. */
const IMAGE_EXTENSIONS = ["jpg", "jpeg", "png", "gif", "webp", "heic", "heif", "bmp"];

/**
 * Which kind a local path is, mirroring `MediaCollection::from_extension` in
 * Rust. Unknown extensions read as video, matching the backend — the UI must
 * not label something a photo that the device will file as a video.
 */
export function mediaKindOf(path: string): MediaCollection {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return IMAGE_EXTENSIONS.includes(ext) ? "image" : "video";
}

export const ldplayerBrowsePath = (kind: "folder" | "file") =>
  invoke<string | null>("ldplayer_browse_path", { kind });

/** Live device and log events. Returns an unlisten function per subscription. */
export function subscribeToDeviceEvents(handlers: {
  onDevices?: (devices: DeviceView[]) => void;
  onDevice?: (device: DeviceView) => void;
  onLog?: (line: LogLine) => void;
}): Promise<UnlistenFn> {
  const pending: Promise<UnlistenFn>[] = [];
  if (handlers.onDevices)
    pending.push(listen<DeviceView[]>("ldplayer://devices", (e) => handlers.onDevices!(e.payload)));
  if (handlers.onDevice)
    pending.push(listen<DeviceView>("ldplayer://device", (e) => handlers.onDevice!(e.payload)));
  if (handlers.onLog)
    pending.push(listen<LogLine>("ldplayer://log", (e) => handlers.onLog!(e.payload)));

  return Promise.all(pending).then((unlisteners) => () => {
    for (const un of unlisteners) un();
  });
}

/** What the UI shows for each device state. Single source, so the dot and the label never disagree. */
export const DEVICE_STATE_LABEL: Record<DeviceState, string> = {
  online: "Connected",
  booting: "Starting…",
  unreachable: "Not responding",
  stopped: "Stopped",
};
