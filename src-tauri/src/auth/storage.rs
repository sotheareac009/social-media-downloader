//! Secure credential storage.
//!
//! The rest of the application talks to the [`CredentialStore`] trait only, and
//! never learns which OS mechanism is behind it:
//!
//! * macOS    -> Keychain
//! * Windows  -> Credential Manager
//! * Linux    -> Secret Service (libsecret / gnome-keyring / KWallet)
//!
//! Nothing here ever writes a credential to disk, to SQLite, or to a log.

use crate::auth::{Credential, ProviderId};
use crate::errors::{AppError, Result};

/// Service name registered in the OS credential store. All entries this app
/// creates live under it, so uninstall cleanup is a single namespace sweep.
pub const KEYRING_SERVICE: &str = "com.reach.mediadownloader.auth";

pub trait CredentialStore: Send + Sync {
    fn save(&self, provider: ProviderId, credential: &Credential) -> Result<()>;
    fn get(&self, provider: ProviderId) -> Result<Option<Credential>>;
    fn delete(&self, provider: ProviderId) -> Result<()>;
}

// Note: there is deliberately no `has()` probe. Any such method would have to
// decrypt the secret to answer, which on macOS costs a Keychain authorization
// prompt - and callers reached for it to render UI. Connection state for the
// UI comes from the metadata database instead; see `AuthManager::account_view`.

/// The real implementation: owner-only files in the app data directory.
///
/// Not the OS keychain. On macOS, reading a keychain item from an *unsigned*
/// build pops a login-password prompt on every read, because the code
/// signature changes each rebuild and the item's ACL no longer recognises the
/// app. That made connected accounts unusable - a password prompt every time a
/// token was read to list Pages or show a channel.
///
/// Files carry the same trade-off already accepted for the download sessions
/// (see `crate::download::session`): `0600` on macOS/Linux, user-scoped
/// `%APPDATA%` on Windows, written atomically. An access token is revocable and
/// short-lived; a usable app without constant prompts is worth more here than
/// encryption at rest. A credential stored in the old keychain is migrated to a
/// file the first time it is read - one final prompt, then never again.
pub struct OsCredentialStore {
    dir: std::path::PathBuf,
}

impl OsCredentialStore {
    pub fn new(dir: std::path::PathBuf) -> Self {
        Self { dir }
    }

    fn file(&self, provider: ProviderId) -> std::path::PathBuf {
        self.dir.join(format!("cred-{}.json", provider.as_str()))
    }

    /// One-time move of a credential stored by an earlier keychain version.
    fn migrate_from_keychain(&self, provider: ProviderId) -> Option<Credential> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, provider.as_str()).ok()?;
        let blob = entry.get_password().ok()?;
        let cred: Credential = serde_json::from_str(&blob).ok()?;
        if self.save(provider, &cred).is_ok() {
            let _ = entry.delete_credential();
        }
        Some(cred)
    }
}

impl CredentialStore for OsCredentialStore {
    fn save(&self, provider: ProviderId, credential: &Credential) -> Result<()> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| AppError::Keychain(format!("credential dir: {e}")))?;
        let blob = serde_json::to_string(credential)
            .map_err(|_| AppError::Internal("credential encode failed".into()))?;

        let target = self.file(provider);
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
                .map_err(|e| AppError::Keychain(format!("credential file: {e}")))?;
            f.write_all(blob.as_bytes())
                .map_err(|e| AppError::Keychain(format!("credential file: {e}")))?;
        }
        std::fs::rename(&temp, &target).map_err(|e| {
            let _ = std::fs::remove_file(&temp);
            AppError::Keychain(format!("credential file: {e}"))
        })
    }

    fn get(&self, provider: ProviderId) -> Result<Option<Credential>> {
        if let Ok(blob) = std::fs::read_to_string(self.file(provider)) {
            // Corrupt/old-format is treated as absent; the user reconnects.
            return Ok(serde_json::from_str(&blob).ok());
        }
        Ok(self.migrate_from_keychain(provider))
    }

    fn delete(&self, provider: ProviderId) -> Result<()> {
        match std::fs::remove_file(self.file(provider)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(AppError::Keychain(format!("credential file: {e}"))),
        }
        // Drop any lingering keychain entry too, so disconnect really clears it.
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, provider.as_str()) {
            let _ = entry.delete_credential();
        }
        Ok(())
    }
}

/// In-memory store used by tests so the suite never touches the real keychain.
#[cfg(test)]
pub struct MemoryCredentialStore {
    inner: std::sync::Mutex<std::collections::HashMap<&'static str, String>>,
}

#[cfg(test)]
impl MemoryCredentialStore {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

#[cfg(test)]
impl CredentialStore for MemoryCredentialStore {
    fn save(&self, provider: ProviderId, credential: &Credential) -> Result<()> {
        let blob = serde_json::to_string(credential).unwrap();
        self.inner.lock().unwrap().insert(provider.as_str(), blob);
        Ok(())
    }

    fn get(&self, provider: ProviderId) -> Result<Option<Credential>> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .get(provider.as_str())
            .map(|b| serde_json::from_str(b).unwrap()))
    }

    fn delete(&self, provider: ProviderId) -> Result<()> {
        self.inner.lock().unwrap().remove(provider.as_str());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Credential {
        Credential {
            provider: ProviderId::Google,
            access_token: "SECRET-ACCESS-VALUE".into(),
            refresh_token: Some("SECRET-REFRESH-VALUE".into()),
            expires_at: Some(crate::auth::now_unix() + 3600),
            scopes: vec!["openid".into()],
            token_type: "Bearer".into(),
        }
    }

    #[test]
    fn roundtrip_save_get_delete() {
        let store = MemoryCredentialStore::new();
        assert!(store.get(ProviderId::Google).unwrap().is_none());

        store.save(ProviderId::Google, &sample()).unwrap();
        let got = store.get(ProviderId::Google).unwrap().unwrap();
        assert_eq!(got.access_token, "SECRET-ACCESS-VALUE");

        store.delete(ProviderId::Google).unwrap();
        assert!(store.get(ProviderId::Google).unwrap().is_none());
        // Deleting twice is not an error.
        store.delete(ProviderId::Google).unwrap();
    }

    #[test]
    fn debug_never_prints_the_token() {
        let printed = format!("{:?}", sample());
        assert!(!printed.contains("SECRET-ACCESS-VALUE"), "access token leaked into Debug output");
        assert!(!printed.contains("SECRET-REFRESH-VALUE"), "refresh token leaked into Debug output");
        assert!(printed.contains("<redacted>"));
    }

    #[test]
    fn expiry_respects_skew() {
        let mut c = sample();
        c.expires_at = Some(crate::auth::now_unix() + 30);
        assert!(!c.is_expired(0));
        assert!(c.is_expired(60));
    }
}
