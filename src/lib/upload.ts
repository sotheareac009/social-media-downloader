/** Bridge to the unified upload commands. Credentials stay in Rust. */
import { invoke } from "@tauri-apps/api/core";

export interface UploadTarget {
  id: string;
  name: string;
  /** "video", "photo", or "video,photo". */
  accepts: string;
  ready: boolean;
  reason: string | null;
}

export type Privacy = "public" | "unlisted" | "private";

export interface YoutubeChannel {
  id: string;
  title: string;
  thumbnail: string | null;
}

export const uploadTargets = () => invoke<UploadTarget[]>("upload_targets");

export const uploadYoutubeChannels = () =>
  invoke<YoutubeChannel[]>("upload_youtube_channels");

export const uploadPickFile = (kind: "video" | "photo" | "any") =>
  invoke<string | null>("upload_pick_file", { kind });

/** Upload a video to YouTube. Resolves to the new video id. */
export const uploadYoutube = (
  filePath: string,
  title: string,
  description: string,
  privacy: Privacy,
) => invoke<string>("upload_youtube", { filePath, title, description, privacy });
