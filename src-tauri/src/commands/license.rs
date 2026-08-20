//! Licence activation commands.
//!
//! The frontend only ever receives the *verified* facts about a licence - plan,
//! expiry, customer tag. The key itself is never returned, so a screenshot of
//! the app cannot hand someone a working key.

use tauri::Manager;

use crate::errors::{AppError, Result};
use crate::license::{self, store};

/// What the UI needs to decide whether to show the app or the activation screen.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LicenseStatus {
    /// False when this build has no public key compiled in, which is the case
    /// for `npm run tauri dev`. The UI treats that as "no gate".
    pub enforced: bool,
    pub activated: bool,
    pub plan: Option<String>,
    /// Unix seconds; absent for a perpetual licence.
    pub expires_at: Option<i64>,
    /// Short id to quote in support, e.g. "6a6c2619".
    pub tag: Option<String>,
}

impl LicenseStatus {
    fn inactive(enforced: bool) -> Self {
        Self {
            enforced,
            activated: false,
            plan: None,
            expires_at: None,
            tag: None,
        }
    }

    fn from(license: &license::License) -> Self {
        Self {
            enforced: true,
            activated: true,
            plan: Some(license.plan.as_str().to_string()),
            expires_at: license.expires_at,
            tag: Some(license.tag_hex()),
        }
    }
}

fn data_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf> {
    app.path()
        .app_data_dir()
        .map_err(|e| AppError::Internal(format!("no app data dir: {e}")))
}

/// Current activation state.
///
/// Re-verifies the stored key every time rather than trusting a cached flag, so
/// an edited licence file grants nothing and an expired licence takes effect the
/// moment it lapses.
#[tauri::command]
pub async fn license_status(app: tauri::AppHandle) -> Result<LicenseStatus> {
    if !license::is_enforced() {
        return Ok(LicenseStatus::inactive(false));
    }
    let dir = data_dir(&app)?;

    match store::load(&dir) {
        None => Ok(LicenseStatus::inactive(true)),
        Some(raw) => match license::verify(&raw) {
            Ok(l) => Ok(LicenseStatus::from(&l)),
            // A stored key that no longer verifies - expired, or from a
            // superseded signing key - is dropped so the user is asked once
            // rather than being told it is broken on every launch.
            Err(_) => {
                let _ = store::clear(&dir);
                Ok(LicenseStatus::inactive(true))
            }
        },
    }
}

/// Validate a pasted key and, if it is good, remember it.
///
/// Nothing is written unless verification succeeds, so a failed attempt cannot
/// leave the app in a half-activated state.
#[tauri::command]
pub async fn license_activate(app: tauri::AppHandle, key: String) -> Result<LicenseStatus> {
    let license = license::verify(&key)?;
    let dir = data_dir(&app)?;
    store::save(&dir, &key)?;
    Ok(LicenseStatus::from(&license))
}

/// Forget the stored key, e.g. to move a licence to another machine.
#[tauri::command]
pub async fn license_deactivate(app: tauri::AppHandle) -> Result<LicenseStatus> {
    store::clear(&data_dir(&app)?)?;
    Ok(LicenseStatus::inactive(license::is_enforced()))
}
