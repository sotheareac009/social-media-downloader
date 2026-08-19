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

export const uploadPickFiles = (kind: "video" | "photo" | "any") =>
  invoke<string[]>("upload_pick_files", { kind });

/** A base64 data-URL poster frame for a video, or null if unavailable. */
export const uploadVideoThumbnail = (path: string) =>
  invoke<string | null>("upload_video_thumbnail", { path });

export interface VideoMeta {
  width: number;
  height: number;
  duration: number;
}

/** Video dimensions + duration (for Telegram video attributes), or null. */
export const uploadVideoMeta = (path: string) =>
  invoke<VideoMeta | null>("upload_video_meta", { path });

/** Upload a video to YouTube. Resolves to the new video id. */
export const uploadYoutube = (
  filePath: string,
  title: string,
  description: string,
  privacy: Privacy,
) => invoke<string>("upload_youtube", { filePath, title, description, privacy });
