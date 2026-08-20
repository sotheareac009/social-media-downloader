//! Multiple YouTube (Google) uploader accounts.
//!
//! The generic auth layer (`auth::manager`) holds exactly one credential per
//! provider, which is right for "sign in to show who you are". Uploading is
//! different: a creator may run several channels under several Google logins
//! and want to push one video to all of them. So YouTube uploader logins live
//! here instead — a flat set of credentials, keyed by the Google account id,
//! entirely separate from the single-account slot on the Accounts page.
//!
//! Each account is one file, `<id>.json`, `0600`, holding the OAuth credential
//! plus the non-secret facts needed to render its card. The credential never
//! reaches the frontend; only [`YoutubeAccountView`] does.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::auth::Credential;
use crate::errors::{AppError, Result};

/// A stored uploader account: secret credential + display metadata.
#[derive(Serialize, Deserialize, Clone)]
pub struct StoredAccount {
    /// Google's stable account id (`sub`). Also the filename stem.
    pub external_id: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub email: Option<String>,
    /// The channel this login uploads to, resolved once at add time.
    pub channel_title: Option<String>,
    pub channel_avatar: Option<String>,
    pub added_at: i64,
    pub credential: Credential,
}

/// The safe half sent to React — no token.
#[derive(Serialize, Clone)]
pub struct YoutubeAccountView {
    pub id: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub email: Option<String>,
    pub channel_title: Option<String>,
    pub channel_avatar: Option<String>,
}

impl From<&StoredAccount> for YoutubeAccountView {
    fn from(a: &StoredAccount) -> Self {
        YoutubeAccountView {
            id: a.external_id.clone(),
            display_name: a.display_name.clone(),
            avatar_url: a.avatar_url.clone(),
            email: a.email.clone(),
            channel_title: a.channel_title.clone(),
            channel_avatar: a.channel_avatar.clone(),
        }
    }
}

/// Folder holding one file per account.
pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("youtube-accounts")
}

/// Keep filenames to a safe, predictable set. Google ids are digits, but a
/// stray character must never escape the directory or collide.
fn safe_stem(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn file(data_dir: &Path, id: &str) -> PathBuf {
    dir(data_dir).join(format!("{}.json", safe_stem(id)))
}

/// Every stored account, newest first. Unreadable files are skipped rather
/// than failing the whole list.
pub fn list(data_dir: &Path) -> Vec<StoredAccount> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir(data_dir)) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(acct) = serde_json::from_str::<StoredAccount>(&text) {
                    out.push(acct);
                }
            }
        }
    }
    out.sort_by(|a, b| b.added_at.cmp(&a.added_at));
    out
}

/// Load one account by id, if present.
pub fn load(data_dir: &Path, id: &str) -> Option<StoredAccount> {
    let text = std::fs::read_to_string(file(data_dir, id)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Write (or overwrite) an account, `0600`, atomically.
pub fn save(data_dir: &Path, account: &StoredAccount) -> Result<()> {
    let folder = dir(data_dir);
    std::fs::create_dir_all(&folder)
        .map_err(|e| AppError::Internal(format!("youtube dir: {e}")))?;
    let blob = serde_json::to_string(account)
        .map_err(|_| AppError::Internal("account encode failed".into()))?;

    let target = file(data_dir, &account.external_id);
    let temp = target.with_extension("json.tmp");

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    {
        use std::io::Write;
        let mut f = options
            .open(&temp)
            .map_err(|e| AppError::Internal(format!("account file: {e}")))?;
        f.write_all(blob.as_bytes())
            .map_err(|e| AppError::Internal(format!("account write: {e}")))?;
    }
    std::fs::rename(&temp, &target)
        .map_err(|e| AppError::Internal(format!("account rename: {e}")))?;
    Ok(())
}

/// Forget an account. Missing file is not an error.
pub fn remove(data_dir: &Path, id: &str) -> Result<()> {
    let target = file(data_dir, id);
    if target.exists() {
        std::fs::remove_file(&target)
            .map_err(|e| AppError::Internal(format!("account delete: {e}")))?;
    }
    Ok(())
}
