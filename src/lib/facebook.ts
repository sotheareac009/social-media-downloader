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

export const facebookRecentDownloads = () =>
  invoke<string[]>("facebook_recent_downloads");
