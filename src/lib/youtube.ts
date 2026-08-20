import { invoke } from "@tauri-apps/api/core";

/** A connected YouTube uploader account (no token). */
export interface YoutubeAccount {
  id: string;
  display_name: string;
  avatar_url: string | null;
  email: string | null;
  channel_title: string | null;
  channel_avatar: string | null;
}

export const youtubeAccountsList = () =>
  invoke<YoutubeAccount[]>("youtube_accounts_list");

/** Opens Google's account chooser and stores the picked account. */
export const youtubeAccountAdd = () => invoke<YoutubeAccount>("youtube_account_add");

export const youtubeAccountRemove = (accountId: string) =>
  invoke<void>("youtube_account_remove", { accountId });

export const youtubeAccountUpload = (
  accountId: string,
  filePath: string,
  title: string,
  description: string,
  privacy: string,
) =>
  invoke<string>("youtube_account_upload", {
    accountId,
    filePath,
    title,
    description,
    privacy,
  });
