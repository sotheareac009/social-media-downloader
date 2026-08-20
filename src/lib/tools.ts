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
}

export const toolsStatus = () => invoke<ToolsStatus>("tools_status");
export const toolsInstall = () => invoke<ToolsStatus>("tools_install");

export const onToolsProgress = (
  cb: (p: ToolsProgress) => void,
): Promise<UnlistenFn> =>
  listen<ToolsProgress>("tools://progress", (e) => cb(e.payload));
