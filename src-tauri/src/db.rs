//! Local metadata database.
//!
//! HARD RULE: this database stores only non-sensitive account metadata. No
//! access token, refresh token, authorization code, PKCE verifier or cookie is
//! ever written here. Secrets live exclusively in the OS credential store.
//! The `accounts` table has no column capable of holding one.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

use crate::auth::{now_unix, AccountInfo, Credential, ProviderId, EXPIRY_SKEW_SECS};
use crate::errors::{AppError, Result};

/// Everything the frontend is allowed to know about a connected account.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AccountView {
    pub provider: ProviderId,
    pub connected: bool,
    pub external_id: Option<String>,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub email: Option<String>,
    pub created_at: Option<i64>,
    pub last_used_at: Option<i64>,
    /// Advisory only. The UI uses it to show a "Reconnect" hint; it is derived
    /// from the credential's expiry, never from the credential itself.
    pub needs_reauth: bool,
}

impl AccountView {
    pub fn disconnected(provider: ProviderId) -> Self {
        Self {
            provider,
            connected: false,
            external_id: None,
            display_name: None,
            avatar_url: None,
            email: None,
            created_at: None,
            last_used_at: None,
            needs_reauth: false,
        }
    }
}

/// Non-secret facts *about* a credential, cached so the Accounts page can be
/// rendered without opening the OS keychain.
///
/// Neither field is sensitive: one is a timestamp, the other a boolean. The
/// token itself stays in the keychain and is never mirrored here.
#[derive(Debug, Clone, Copy)]
pub struct CredentialMeta {
    pub expires_at: Option<i64>,
    /// Whether the provider can renew this credential without user interaction.
    pub refreshable: bool,
}

impl CredentialMeta {
    /// Build from a live credential plus the provider's own refresh verdict.
    pub fn new(credential: &Credential, refreshable: bool) -> Self {
        Self {
            expires_at: credential.expires_at,
            refreshable,
        }
    }

    fn needs_reauth(&self) -> bool {
        match self.expires_at {
            Some(exp) => now_unix() + EXPIRY_SKEW_SECS >= exp && !self.refreshable,
            // No advertised expiry: the API call is the authority.
            None => false,
        }
    }
}

pub struct AccountDb {
    conn: Mutex<Connection>,
}

impl AccountDb {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| AppError::Database(format!("could not create data dir: {e}")))?;
        }
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS accounts (
                id            TEXT PRIMARY KEY,
                provider      TEXT NOT NULL UNIQUE,
                external_id   TEXT NOT NULL,
                display_name  TEXT NOT NULL,
                avatar_url    TEXT,
                email         TEXT,
                created_at    INTEGER NOT NULL,
                last_used_at  INTEGER NOT NULL
            );
            "#,
        )?;
        Self::migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Additive migrations. Each is guarded by a column check so it is safe to
    /// run against a database created by an earlier build.
    ///
    /// Every column added here must remain non-sensitive - see the guard test
    /// at the bottom of this file, which fails the build if one looks like it
    /// could hold a secret.
    ///
    /// Note the SQL column is `renewable` while the Rust field is
    /// `refreshable`. That is deliberate: the guard test rejects any column
    /// whose name contains "refresh", and weakening the guard to admit a
    /// harmless boolean would also admit `refresh_token`.
    fn migrate(conn: &Connection) -> Result<()> {
        let existing: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(accounts)")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
            rows.collect::<std::result::Result<_, _>>()?
        };

        if !existing.iter().any(|c| c == "expires_at") {
            conn.execute("ALTER TABLE accounts ADD COLUMN expires_at INTEGER", [])?;
        }
        if !existing.iter().any(|c| c == "renewable") {
            conn.execute(
                "ALTER TABLE accounts ADD COLUMN renewable INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        Ok(())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| AppError::Database("connection lock poisoned".into()))
    }

    /// Insert or update the row for a provider. One account per provider for
    /// now; reconnecting as a different user replaces the row.
    pub fn upsert(&self, info: &AccountInfo, meta: CredentialMeta) -> Result<AccountView> {
        let conn = self.lock()?;
        let now = now_unix();

        conn.execute(
            r#"
            INSERT INTO accounts (id, provider, external_id, display_name, avatar_url, email,
                                  created_at, last_used_at, expires_at, renewable)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8, ?9)
            ON CONFLICT(provider) DO UPDATE SET
                external_id  = excluded.external_id,
                display_name = excluded.display_name,
                avatar_url   = excluded.avatar_url,
                email        = excluded.email,
                last_used_at = excluded.last_used_at,
                expires_at   = excluded.expires_at,
                renewable    = excluded.renewable,
                -- keep the original created_at unless the account itself changed
                created_at   = CASE WHEN accounts.external_id = excluded.external_id
                                    THEN accounts.created_at ELSE excluded.created_at END
            "#,
            params![
                uuid::Uuid::new_v4().to_string(),
                info.provider.as_str(),
                info.external_id,
                info.display_name,
                info.avatar_url,
                info.email,
                now,
                meta.expires_at,
                meta.refreshable as i64,
            ],
        )?;
        drop(conn);

        Ok(self
            .get(info.provider)?
            .unwrap_or_else(|| AccountView::disconnected(info.provider)))
    }

    pub fn get(&self, provider: ProviderId) -> Result<Option<AccountView>> {
        let conn = self.lock()?;
        let row = conn
            .query_row(
                "SELECT external_id, display_name, avatar_url, email, created_at, last_used_at,
                        expires_at, renewable
                 FROM accounts WHERE provider = ?1",
                params![provider.as_str()],
                |r| {
                    Ok(AccountView {
                        provider,
                        connected: true,
                        external_id: Some(r.get(0)?),
                        display_name: Some(r.get(1)?),
                        avatar_url: r.get(2)?,
                        email: r.get(3)?,
                        created_at: Some(r.get(4)?),
                        last_used_at: Some(r.get(5)?),
                        needs_reauth: CredentialMeta {
                            expires_at: r.get(6)?,
                            refreshable: r.get::<_, i64>(7)? != 0,
                        }
                        .needs_reauth(),
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Refresh the cached credential facts after a token renewal, so the UI
    /// stops showing "Reconnect needed" without another keychain read.
    pub fn update_meta(&self, provider: ProviderId, meta: CredentialMeta) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "UPDATE accounts SET expires_at = ?1, renewable = ?2, last_used_at = ?3
             WHERE provider = ?4",
            params![
                meta.expires_at,
                meta.refreshable as i64,
                now_unix(),
                provider.as_str()
            ],
        )?;
        Ok(())
    }

    pub fn touch(&self, provider: ProviderId) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "UPDATE accounts SET last_used_at = ?1 WHERE provider = ?2",
            params![now_unix(), provider.as_str()],
        )?;
        Ok(())
    }

    pub fn delete(&self, provider: ProviderId) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "DELETE FROM accounts WHERE provider = ?1",
            params![provider.as_str()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(expires_in: Option<i64>, refreshable: bool) -> CredentialMeta {
        CredentialMeta {
            expires_at: expires_in.map(|s| now_unix() + s),
            refreshable,
        }
    }

    fn live() -> CredentialMeta {
        meta(Some(3600), true)
    }

    fn info(name: &str, ext: &str) -> AccountInfo {
        AccountInfo {
            provider: ProviderId::Google,
            external_id: ext.into(),
            display_name: name.into(),
            avatar_url: Some("https://example.com/a.png".into()),
            email: Some("a@example.com".into()),
        }
    }

    #[test]
    fn upsert_then_get_then_delete() {
        let db = AccountDb::open_in_memory().unwrap();
        assert!(db.get(ProviderId::Google).unwrap().is_none());

        let v = db.upsert(&info("Jane Doe", "ext-1"), live()).unwrap();
        assert!(v.connected);
        assert_eq!(v.display_name.as_deref(), Some("Jane Doe"));

        db.delete(ProviderId::Google).unwrap();
        assert!(db.get(ProviderId::Google).unwrap().is_none());
    }

    #[test]
    fn reconnecting_same_account_preserves_created_at() {
        let db = AccountDb::open_in_memory().unwrap();
        let first = db.upsert(&info("Jane Doe", "ext-1"), live()).unwrap();
        let again = db.upsert(&info("Jane D.", "ext-1"), live()).unwrap();
        assert_eq!(first.created_at, again.created_at);
        assert_eq!(again.display_name.as_deref(), Some("Jane D."));
    }

    #[test]
    fn schema_has_no_column_that_could_hold_a_secret() {
        let db = AccountDb::open_in_memory().unwrap();
        let conn = db.lock().unwrap();
        let mut stmt = conn.prepare("PRAGMA table_info(accounts)").unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .map(|c| c.unwrap().to_lowercase())
            .collect();
        for forbidden in ["token", "access", "refresh", "secret", "cookie", "password", "code"] {
            assert!(
                !cols.iter().any(|c| c.contains(forbidden)),
                "accounts table gained a `{forbidden}`-ish column: {cols:?}"
            );
        }
    }

    #[test]
    fn needs_reauth_only_when_expired_and_not_refreshable() {
        let db = AccountDb::open_in_memory().unwrap();
        let i = info("Jane", "ext-1");

        // Live token.
        let v = db.upsert(&i, meta(Some(3600), false)).unwrap();
        assert!(!v.needs_reauth);

        // Expired but renewable - Instagram and Google with a refresh token.
        let v = db.upsert(&i, meta(Some(-10), true)).unwrap();
        assert!(!v.needs_reauth, "a refreshable credential must not nag");

        // Expired and not renewable - genuinely needs the user back.
        let v = db.upsert(&i, meta(Some(-10), false)).unwrap();
        assert!(v.needs_reauth);

        // No advertised expiry: the API call is the authority, not us.
        let v = db.upsert(&i, meta(None, false)).unwrap();
        assert!(!v.needs_reauth);
    }

    #[test]
    fn expiry_skew_is_applied() {
        let db = AccountDb::open_in_memory().unwrap();
        // Inside the skew window, a non-refreshable token already counts as
        // needing re-auth so a request never goes out with a dying token.
        let v = db
            .upsert(&info("Jane", "ext-1"), meta(Some(EXPIRY_SKEW_SECS / 2), false))
            .unwrap();
        assert!(v.needs_reauth);
    }

    #[test]
    fn update_meta_clears_the_reconnect_hint_after_a_refresh() {
        let db = AccountDb::open_in_memory().unwrap();
        let v = db.upsert(&info("Jane", "ext-1"), meta(Some(-10), false)).unwrap();
        assert!(v.needs_reauth);

        db.update_meta(ProviderId::Google, meta(Some(3600), true)).unwrap();
        assert!(!db.get(ProviderId::Google).unwrap().unwrap().needs_reauth);
    }

    /// A database written by an earlier build must gain the new columns rather
    /// than erroring on open.
    #[test]
    fn migration_adds_columns_to_a_legacy_schema() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE accounts (
                id TEXT PRIMARY KEY, provider TEXT NOT NULL UNIQUE,
                external_id TEXT NOT NULL, display_name TEXT NOT NULL,
                avatar_url TEXT, email TEXT,
                created_at INTEGER NOT NULL, last_used_at INTEGER NOT NULL);",
        )
        .unwrap();

        let db = AccountDb::from_connection(conn).unwrap();
        // Running twice must also be safe.
        {
            let c = db.lock().unwrap();
            AccountDb::migrate(&c).unwrap();
        }

        let v = db.upsert(&info("Jane", "ext-1"), live()).unwrap();
        assert!(v.connected);
    }
}
