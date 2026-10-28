/** Bridge to Facebook Page publishing. Tokens stay in Rust. */
import { invoke } from "@tauri-apps/api/core";

export interface Page {
  id: string;
  name: string;
  avatar_url: string | null;
}

export const facebookListPages = () => invoke<Page[]>("facebook_list_pages");

/** Opens an image picker; resolves to a path, or null if dismissed. */
export const facebookPickPhoto = () =>
  invoke<string | null>("facebook_pick_photo");

/** Publish a photo to a Page. Resolves to the new post id. */
export const facebookUploadPhoto = (
  pageId: string,
  filePath: string,
  caption: string,
) =>
  invoke<string>("facebook_upload_photo", { pageId, filePath, caption });

/** Publish a video to a Page (chunked in Rust). Resolves to the video id. */
export const facebookUploadVideo = (
  pageId: string,
  filePath: string,
  title: string,
  description: string,
) =>
  invoke<string>("facebook_upload_video", { pageId, filePath, title, description });

/** Publish a text-only status to a Page. Resolves to the new post id. */
export const facebookPostStatus = (pageId: string, message: string) =>
  invoke<string>("facebook_post_status", { pageId, message });

export const facebookRecentDownloads = () =>
  invoke<string[]>("facebook_recent_downloads");
