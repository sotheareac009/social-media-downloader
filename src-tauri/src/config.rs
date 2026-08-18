//! Application configuration loading.
//!
//! IMPORTANT distinction: a `.env` file here holds *application* configuration
//! - OAuth client IDs, client secrets and redirect URIs issued to this app by a
//! platform. It never holds a *user's* credential. User access tokens and
//! refresh tokens go to the OS keychain via [`crate::auth::storage`] and are
//! never written to a file. The "no credentials in env files" rule is about
//! user tokens, and it still holds.
//!
//! Lookup priority for any key, highest first:
//!   1. a real process environment variable
//!   2. a `.env` file (loaded into the process env at startup)
//!   3. a value baked in at compile time under the same name
//!
//! `dotenvy` never overwrites a variable that is already set, so (1) beating
//! (2) is automatic.

use std::path::PathBuf;

/// Load a `.env` file into the process environment, if one can be found.
///
/// Returns the path that was loaded, for the startup diagnostic. Called once,
/// before any provider is constructed.
pub fn load_dotenv() -> Option<PathBuf> {
    for path in candidate_paths() {
        if path.is_file() && dotenvy::from_path(&path).is_ok() {
            return Some(path);
        }
    }
    None
}

fn candidate_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // An explicit override always wins - useful for CI and for pointing a
    // packaged build at a file outside the bundle.
    if let Ok(explicit) = std::env::var("MEDIA_DOWNLOADER_ENV_FILE") {
        paths.push(PathBuf::from(explicit));
    }

    // The working directory, and its parent. Under `npm run tauri dev` the
    // process starts in `src-tauri/`, so the parent is the project root where
    // people naturally put `.env`.
    if let Ok(cwd) = std::env::current_dir() {
        paths.push(cwd.join(".env"));
        if let Some(parent) = cwd.parent() {
            paths.push(parent.join(".env"));
        }
    }

    // Fixed dev locations, independent of where the binary was invoked from.
    // `CARGO_MANIFEST_DIR` is baked in at compile time, so in a shipped bundle
    // these simply will not exist and are skipped.
    if cfg!(debug_assertions) {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        paths.push(manifest.join(".env"));
        if let Some(root) = manifest.parent() {
            paths.push(root.join(".env"));
        }
    }

    paths.dedup();
    paths
}

/// Read one configuration key.
///
/// SECURITY: the value may be a client secret, so it is returned to the caller
/// and never printed, logged or included in an error.
pub fn read(key: &str) -> Option<String> {
    if let Ok(v) = std::env::var(key) {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return Some(v);
        }
    }

    // `option_env!` needs a literal, so the supported keys are enumerated.
    match key {
        "GOOGLE_CLIENT_ID" => option_env!("GOOGLE_CLIENT_ID"),
        "GOOGLE_CLIENT_SECRET" => option_env!("GOOGLE_CLIENT_SECRET"),
        "FACEBOOK_CLIENT_ID" => option_env!("FACEBOOK_CLIENT_ID"),
        "FACEBOOK_CLIENT_SECRET" => option_env!("FACEBOOK_CLIENT_SECRET"),
        "FACEBOOK_REDIRECT_URI" => option_env!("FACEBOOK_REDIRECT_URI"),
        "TIKTOK_CLIENT_KEY" => option_env!("TIKTOK_CLIENT_KEY"),
        "TIKTOK_CLIENT_SECRET" => option_env!("TIKTOK_CLIENT_SECRET"),
        "INSTAGRAM_CLIENT_ID" => option_env!("INSTAGRAM_CLIENT_ID"),
        "INSTAGRAM_CLIENT_SECRET" => option_env!("INSTAGRAM_CLIENT_SECRET"),
        "INSTAGRAM_REDIRECT_URI" => option_env!("INSTAGRAM_REDIRECT_URI"),
        // Not an OAuth value: an explicit path to the yt-dlp binary, for the
        // common case where a GUI app doesn't inherit the shell's PATH.
        "MEDIA_DOWNLOADER_YTDLP" => option_env!("MEDIA_DOWNLOADER_YTDLP"),
        _ => None,
    }
    .map(str::to_string)
    .filter(|v| !v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_key_is_none() {
        assert!(read("MEDIA_DOWNLOADER_DEFINITELY_UNSET_KEY").is_none());
    }

    #[test]
    fn blank_and_whitespace_values_count_as_unset() {
        // A `.env` line like `FACEBOOK_CLIENT_ID=` must leave the provider
        // unconfigured rather than half-configured with an empty id.
        std::env::set_var("MEDIA_DOWNLOADER_TEST_BLANK", "   ");
        assert!(read("MEDIA_DOWNLOADER_TEST_BLANK").is_none());
        std::env::remove_var("MEDIA_DOWNLOADER_TEST_BLANK");
    }

    #[test]
    fn values_are_trimmed() {
        std::env::set_var("MEDIA_DOWNLOADER_TEST_PADDED", "  value  ");
        assert_eq!(read("MEDIA_DOWNLOADER_TEST_PADDED").as_deref(), Some("value"));
        std::env::remove_var("MEDIA_DOWNLOADER_TEST_PADDED");
    }

    /// End-to-end: a real file on disk reaches `read`, and a real environment
    /// variable still beats it.
    #[test]
    fn dotenv_file_is_loaded_and_env_wins_over_it() {
        let dir = std::env::temp_dir().join("md-config-test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(".env");
        std::fs::write(
            &file,
            "MD_TEST_FROM_FILE=file-value\nMD_TEST_OVERRIDDEN=file-value\n",
        )
        .unwrap();

        // Pre-set one of the two keys: dotenvy must not clobber it.
        std::env::set_var("MD_TEST_OVERRIDDEN", "env-value");
        std::env::set_var("MEDIA_DOWNLOADER_ENV_FILE", &file);

        let loaded = load_dotenv();
        assert_eq!(loaded.as_deref(), Some(file.as_path()));

        assert_eq!(read("MD_TEST_FROM_FILE").as_deref(), Some("file-value"));
        assert_eq!(read("MD_TEST_OVERRIDDEN").as_deref(), Some("env-value"));

        for k in ["MD_TEST_FROM_FILE", "MD_TEST_OVERRIDDEN", "MEDIA_DOWNLOADER_ENV_FILE"] {
            std::env::remove_var(k);
        }
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn candidate_paths_include_cwd_and_parent() {
        let paths = candidate_paths();
        assert!(paths.iter().any(|p| p.ends_with(".env")));
        assert!(paths.len() >= 2);
    }
}
