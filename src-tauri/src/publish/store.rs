//! Persistence for accounts, staged media and publishing jobs.
//!
//! A separate SQLite file from `accounts.sqlite3` on purpose. That database
//! belongs to the OAuth layer and carries a hard rule — one row per provider,
//! no column capable of holding a secret. This feature has a different shape
//! (many accounts per platform) and a different lifecycle, and joining them
//! would mean one schema serving two rules badly.
//!
//! The no-secrets rule still applies here, and is enforced the same way: the
//! guard test at the bottom fails the build if a column appears whose name
//! suggests it could hold a password, token or cookie. There is no legitimate
//! reason for one — the whole point of the design is that the login stays
//! inside the Android app.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::errors::{AppError, Result};
use crate::publish::model::{Account, JobStatus, MediaItem, Platform, PublishJob};

/// A job as it is stored, before the account and media names are joined on.
#[derive(Debug, Clone)]
pub struct JobRow {
    pub id: String,
    pub media_id: String,
    pub account_id: String,
    pub caption: String,
    pub status: JobStatus,
    pub progress: f64,
    pub step: Option<String>,
    pub error_code: Option<String>,
    pub error: Option<String>,
    pub screenshot_path: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
}

pub struct PublishStore {
    conn: Mutex<Connection>,
}

impl PublishStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| AppError::Database(format!("could not create data dir: {e}")))?;
        }
        Self::from_connection(Connection::open(path)?)
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
            CREATE TABLE IF NOT EXISTS publish_accounts (
                id                   TEXT PRIMARY KEY,
                name                 TEXT NOT NULL,
                platform             TEXT NOT NULL,
                ldplayer_instance_id TEXT NOT NULL,
                package_name         TEXT NOT NULL,
                created_at           INTEGER NOT NULL,
                -- One app per platform per device. A second Facebook account
                -- means a second LDPlayer instance, which is exactly how the
                -- user already thinks about it.
                UNIQUE (ldplayer_instance_id, package_name)
            );

            CREATE TABLE IF NOT EXISTS publish_media (
                id               TEXT PRIMARY KEY,
                path             TEXT NOT NULL,
                file_name        TEXT NOT NULL,
                size_bytes       INTEGER NOT NULL,
                duration_seconds REAL,
                added_at         INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS publish_jobs (
                id              TEXT PRIMARY KEY,
                media_id        TEXT NOT NULL REFERENCES publish_media(id) ON DELETE CASCADE,
                account_id      TEXT NOT NULL REFERENCES publish_accounts(id) ON DELETE CASCADE,
                caption         TEXT NOT NULL,
                status          TEXT NOT NULL,
                progress        REAL NOT NULL DEFAULT 0,
                step            TEXT,
                error_code      TEXT,
                error           TEXT,
                screenshot_path TEXT,
                created_at      INTEGER NOT NULL,
                started_at      INTEGER,
                completed_at    INTEGER
            );

            CREATE INDEX IF NOT EXISTS idx_jobs_status  ON publish_jobs(status);
            CREATE INDEX IF NOT EXISTS idx_jobs_created ON publish_jobs(created_at DESC);
            "#,
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| AppError::Database("connection lock poisoned".into()))
    }

    /// Jobs left mid-flight by a crash or a force-quit.
    ///
    /// Called once at startup. A row still saying "Uploading" from last week
    /// would sit in the UI forever with nothing driving it, so it is failed
    /// honestly instead — the video may or may not have gone out, and saying
    /// "unknown" is the only truthful answer.
    pub fn fail_interrupted(&self) -> Result<usize> {
        let conn = self.lock()?;
        let n = conn.execute(
            "UPDATE publish_jobs
                SET status = 'failed',
                    error_code = 'interrupted',
                    error = 'The app closed while this job was running; its outcome is unknown.',
                    completed_at = ?1
              WHERE status IN ('pending', 'uploading', 'publishing')",
            params![now_unix()],
        )?;
        Ok(n)
    }

    // ----------------------------------------------------------- accounts

    pub fn add_account(
        &self,
        name: &str,
        platform: Platform,
        device_id: &str,
        package: &str,
    ) -> Result<Account> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO publish_accounts
                 (id, name, platform, ldplayer_instance_id, package_name, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(ldplayer_instance_id, package_name) DO UPDATE SET name = excluded.name",
            params![
                uuid::Uuid::new_v4().to_string(),
                name,
                platform.as_str(),
                device_id,
                package,
                now_unix(),
            ],
        )?;
        drop(conn);
        // The upsert may have kept an older row, so read back rather than
        // returning a struct built with a fresh id that was never stored.
        self.account_for(device_id, package)?
            .ok_or_else(|| AppError::Database("account vanished after insert".into()))
    }

    fn account_for(&self, device_id: &str, package: &str) -> Result<Option<Account>> {
        let conn = self.lock()?;
        conn.query_row(
            "SELECT id, name, platform, ldplayer_instance_id, package_name, created_at
               FROM publish_accounts WHERE ldplayer_instance_id = ?1 AND package_name = ?2",
            params![device_id, package],
            account_from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn accounts(&self) -> Result<Vec<Account>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, platform, ldplayer_instance_id, package_name, created_at
               FROM publish_accounts ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], account_from_row)?;
        rows.collect::<std::result::Result<_, _>>().map_err(Into::into)
    }

    pub fn account(&self, id: &str) -> Result<Account> {
        let conn = self.lock()?;
        conn.query_row(
            "SELECT id, name, platform, ldplayer_instance_id, package_name, created_at
               FROM publish_accounts WHERE id = ?1",
            params![id],
            account_from_row,
        )
        .optional()?
        .ok_or_else(|| AppError::AccountNotFound(id.to_string()))
    }

    pub fn rename_account(&self, id: &str, name: &str) -> Result<()> {
        let conn = self.lock()?;
        let n = conn.execute(
            "UPDATE publish_accounts SET name = ?1 WHERE id = ?2",
            params![name, id],
        )?;
        if n == 0 {
            return Err(AppError::AccountNotFound(id.to_string()));
        }
        Ok(())
    }

    pub fn remove_account(&self, id: &str) -> Result<()> {
        let conn = self.lock()?;
        conn.execute("DELETE FROM publish_accounts WHERE id = ?1", params![id])?;
        Ok(())
    }

    // -------------------------------------------------------------- media

    pub fn add_media(&self, item: &MediaItem) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT OR REPLACE INTO publish_media
                 (id, path, file_name, size_bytes, duration_seconds, added_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                item.id,
                item.path,
                item.file_name,
                item.size_bytes as i64,
                item.duration_seconds,
                item.added_at
            ],
        )?;
        Ok(())
    }

    pub fn media(&self, id: &str) -> Result<MediaItem> {
        let conn = self.lock()?;
        conn.query_row(
            "SELECT id, path, file_name, size_bytes, duration_seconds, added_at
               FROM publish_media WHERE id = ?1",
            params![id],
            |r| {
                Ok(MediaItem {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    file_name: r.get(2)?,
                    size_bytes: r.get::<_, i64>(3)? as u64,
                    duration_seconds: r.get(4)?,
                    added_at: r.get(5)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| AppError::MediaFileMissing(id.to_string()))
    }

    // --------------------------------------------------------------- jobs

    pub fn add_job(&self, job: &JobRow) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO publish_jobs
                 (id, media_id, account_id, caption, status, progress, step,
                  error_code, error, screenshot_path, created_at, started_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                job.id,
                job.media_id,
                job.account_id,
                job.caption,
                job.status.as_str(),
                job.progress,
                job.step,
                job.error_code,
                job.error,
                job.screenshot_path,
                job.created_at,
                job.started_at,
                job.completed_at
            ],
        )?;
        Ok(())
    }

    pub fn job(&self, id: &str) -> Result<JobRow> {
        let conn = self.lock()?;
        conn.query_row(
            "SELECT id, media_id, account_id, caption, status, progress, step,
                    error_code, error, screenshot_path, created_at, started_at, completed_at
               FROM publish_jobs WHERE id = ?1",
            params![id],
            job_from_row,
        )
        .optional()?
        .ok_or_else(|| AppError::PublishJobNotFound(id.to_string()))
    }

    /// Newest first — a publishing queue is read from the top.
    pub fn jobs(&self, limit: usize) -> Result<Vec<JobRow>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, media_id, account_id, caption, status, progress, step,
                    error_code, error, screenshot_path, created_at, started_at, completed_at
               FROM publish_jobs ORDER BY created_at DESC, rowid DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], job_from_row)?;
        rows.collect::<std::result::Result<_, _>>().map_err(Into::into)
    }

    /// Write a job's live state. One statement for every transition, so there
    /// is a single place where a job's row can change.
    pub fn update_job(
        &self,
        id: &str,
        status: JobStatus,
        progress: f64,
        step: Option<&str>,
        error_code: Option<&str>,
        error: Option<&str>,
    ) -> Result<()> {
        let conn = self.lock()?;
        let now = now_unix();
        let started = matches!(status, JobStatus::Uploading).then_some(now);
        let completed = status.is_terminal().then_some(now);
        conn.execute(
            "UPDATE publish_jobs
                SET status = ?1,
                    progress = ?2,
                    step = ?3,
                    error_code = ?4,
                    error = ?5,
                    started_at = COALESCE(started_at, ?6),
                    completed_at = ?7
              WHERE id = ?8",
            params![status.as_str(), progress, step, error_code, error, started, completed, id],
        )?;
        Ok(())
    }

    pub fn set_job_screenshot(&self, id: &str, path: &str) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "UPDATE publish_jobs SET screenshot_path = ?1 WHERE id = ?2",
            params![path, id],
        )?;
        Ok(())
    }

    /// Put a stopped job back in the queue. Clears the previous failure so the
    /// UI does not show a stale error beside a running job.
    pub fn reset_job(&self, id: &str) -> Result<JobRow> {
        let job = self.job(id)?;
        if !job.status.is_retryable() {
            return Err(AppError::JobNotRetryable(job.status.as_str().to_string()));
        }
        let conn = self.lock()?;
        conn.execute(
            "UPDATE publish_jobs
                SET status = 'pending', progress = 0, step = NULL,
                    error_code = NULL, error = NULL,
                    started_at = NULL, completed_at = NULL
              WHERE id = ?1",
            params![id],
        )?;
        drop(conn);
        self.job(id)
    }

    pub fn remove_job(&self, id: &str) -> Result<()> {
        let conn = self.lock()?;
        conn.execute("DELETE FROM publish_jobs WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn clear_finished(&self) -> Result<usize> {
        let conn = self.lock()?;
        Ok(conn.execute(
            "DELETE FROM publish_jobs WHERE status IN ('published', 'failed', 'cancelled')",
            [],
        )?)
    }

    /// Counts by status, for the dashboard. One query rather than loading
    /// every job just to count them.
    pub fn counts(&self) -> Result<Vec<(String, i64)>> {
        let conn = self.lock()?;
        let mut stmt =
            conn.prepare("SELECT status, COUNT(*) FROM publish_jobs GROUP BY status")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<std::result::Result<_, _>>().map_err(Into::into)
    }

    /// Join a stored row with the names the UI renders, so the frontend never
    /// has to hold three lists to draw one.
    pub fn view(&self, row: JobRow) -> Result<PublishJob> {
        let account = self.account(&row.account_id).ok();
        let media = self.media(&row.media_id).ok();
        Ok(PublishJob {
            account_name: account
                .as_ref()
                .map(|a| a.name.clone())
                .unwrap_or_else(|| "(deleted account)".into()),
            platform: account.as_ref().map(|a| a.platform).unwrap_or(Platform::Facebook),
            device_id: account
                .as_ref()
                .map(|a| a.ldplayer_instance_id.clone())
                .unwrap_or_default(),
            media_name: media
                .as_ref()
                .map(|m| m.file_name.clone())
                .unwrap_or_else(|| "(missing file)".into()),
            id: row.id,
            media_id: row.media_id,
            account_id: row.account_id,
            caption: row.caption,
            status: row.status,
            progress: row.progress,
            step: row.step,
            error_code: row.error_code,
            error: row.error,
            screenshot_path: row.screenshot_path,
            created_at: row.created_at,
            started_at: row.started_at,
            completed_at: row.completed_at,
        })
    }
}

fn account_from_row(r: &Row<'_>) -> rusqlite::Result<Account> {
    let platform: String = r.get(2)?;
    Ok(Account {
        id: r.get(0)?,
        name: r.get(1)?,
        // An unknown platform string can only come from a hand-edited database
        // or a downgrade. Falling back beats refusing to list every account.
        platform: Platform::parse(&platform).unwrap_or(Platform::Facebook),
        ldplayer_instance_id: r.get(3)?,
        package_name: r.get(4)?,
        created_at: r.get(5)?,
    })
}

fn job_from_row(r: &Row<'_>) -> rusqlite::Result<JobRow> {
    let status: String = r.get(4)?;
    Ok(JobRow {
        id: r.get(0)?,
        media_id: r.get(1)?,
        account_id: r.get(2)?,
        caption: r.get(3)?,
        status: JobStatus::parse(&status).unwrap_or(JobStatus::Failed),
        progress: r.get(5)?,
        step: r.get(6)?,
        error_code: r.get(7)?,
        error: r.get(8)?,
        screenshot_path: r.get(9)?,
        created_at: r.get(10)?,
        started_at: r.get(11)?,
        completed_at: r.get(12)?,
    })
}

pub fn now_unix() -> i64 {
    crate::ldplayer::manager::now_unix()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> PublishStore {
        PublishStore::open_in_memory().unwrap()
    }

    fn media(store: &PublishStore) -> MediaItem {
        let m = MediaItem {
            id: uuid::Uuid::new_v4().to_string(),
            path: "/tmp/my-video.mp4".into(),
            file_name: "my-video.mp4".into(),
            size_bytes: 1024,
            duration_seconds: Some(12.5),
            added_at: now_unix(),
        };
        store.add_media(&m).unwrap();
        m
    }

    fn job(store: &PublishStore, account: &Account, media: &MediaItem) -> JobRow {
        let row = JobRow {
            id: uuid::Uuid::new_v4().to_string(),
            media_id: media.id.clone(),
            account_id: account.id.clone(),
            caption: "Check out my new video!".into(),
            status: JobStatus::Pending,
            progress: 0.0,
            step: None,
            error_code: None,
            error: None,
            screenshot_path: None,
            created_at: now_unix(),
            started_at: None,
            completed_at: None,
        };
        store.add_job(&row).unwrap();
        row
    }

    #[test]
    fn accounts_are_unique_per_device_and_package() {
        let s = store();
        let a = s.add_account("Facebook #1", Platform::Facebook, "ld:0", "com.facebook.katana").unwrap();
        let b = s.add_account("Renamed", Platform::Facebook, "ld:0", "com.facebook.katana").unwrap();
        assert_eq!(a.id, b.id, "the same app on the same device is one account");
        assert_eq!(b.name, "Renamed");
        assert_eq!(s.accounts().unwrap().len(), 1);
    }

    #[test]
    fn the_same_app_on_two_instances_is_two_accounts() {
        let s = store();
        s.add_account("IG #1", Platform::Instagram, "ld:1", "com.instagram.android").unwrap();
        s.add_account("IG #2", Platform::Instagram, "ld:2", "com.instagram.android").unwrap();
        assert_eq!(s.accounts().unwrap().len(), 2);
    }

    #[test]
    fn a_job_moves_through_its_lifecycle_and_records_timestamps() {
        let s = store();
        let a = s.add_account("TikTok", Platform::Tiktok, "ld:3", "com.zhiliaoapp.musically").unwrap();
        let m = media(&s);
        let j = job(&s, &a, &m);

        s.update_job(&j.id, JobStatus::Uploading, 0.3, Some("Copying video"), None, None).unwrap();
        let row = s.job(&j.id).unwrap();
        assert!(row.started_at.is_some());
        assert!(row.completed_at.is_none());

        s.update_job(&j.id, JobStatus::Published, 1.0, Some("Published"), None, None).unwrap();
        let row = s.job(&j.id).unwrap();
        assert_eq!(row.status, JobStatus::Published);
        assert!(row.completed_at.is_some());
    }

    #[test]
    fn started_at_is_not_overwritten_by_a_later_transition() {
        let s = store();
        let a = s.add_account("FB", Platform::Facebook, "ld:0", "com.facebook.katana").unwrap();
        let m = media(&s);
        let j = job(&s, &a, &m);
        s.update_job(&j.id, JobStatus::Uploading, 0.3, None, None, None).unwrap();
        let first = s.job(&j.id).unwrap().started_at.unwrap();
        s.update_job(&j.id, JobStatus::Publishing, 0.6, None, None, None).unwrap();
        assert_eq!(s.job(&j.id).unwrap().started_at, Some(first));
    }

    #[test]
    fn a_failed_job_can_be_retried_and_a_published_one_cannot() {
        let s = store();
        let a = s.add_account("FB", Platform::Facebook, "ld:0", "com.facebook.katana").unwrap();
        let m = media(&s);
        let j = job(&s, &a, &m);

        s.update_job(&j.id, JobStatus::Failed, 0.5, None, Some("adb_failed"), Some("boom")).unwrap();
        let back = s.reset_job(&j.id).unwrap();
        assert_eq!(back.status, JobStatus::Pending);
        assert!(back.error.is_none(), "a retried job must not show the old error");

        s.update_job(&j.id, JobStatus::Published, 1.0, None, None, None).unwrap();
        assert!(s.reset_job(&j.id).is_err(), "retrying a published job would double-post");
    }

    #[test]
    fn interrupted_jobs_are_failed_at_startup() {
        let s = store();
        let a = s.add_account("FB", Platform::Facebook, "ld:0", "com.facebook.katana").unwrap();
        let m = media(&s);
        let j = job(&s, &a, &m);
        s.update_job(&j.id, JobStatus::Publishing, 0.6, None, None, None).unwrap();

        assert_eq!(s.fail_interrupted().unwrap(), 1);
        let row = s.job(&j.id).unwrap();
        assert_eq!(row.status, JobStatus::Failed);
        assert_eq!(row.error_code.as_deref(), Some("interrupted"));
    }

    #[test]
    fn deleting_an_account_takes_its_jobs_with_it() {
        let s = store();
        let a = s.add_account("FB", Platform::Facebook, "ld:0", "com.facebook.katana").unwrap();
        let m = media(&s);
        let j = job(&s, &a, &m);
        s.remove_account(&a.id).unwrap();
        assert!(s.job(&j.id).is_err());
    }

    #[test]
    fn a_job_view_carries_the_names_the_ui_renders() {
        let s = store();
        let a = s.add_account("FB", Platform::Facebook, "ld:0", "com.facebook.katana").unwrap();
        let m = media(&s);
        let row = job(&s, &a, &m);
        let view = s.view(row).unwrap();
        assert_eq!(view.media_name, "my-video.mp4");
        assert_eq!(view.account_name, "FB");
    }

    /// The rule from this module's header, enforced.
    #[test]
    fn no_table_has_a_column_that_could_hold_a_credential() {
        let s = store();
        let conn = s.lock().unwrap();
        for table in ["publish_accounts", "publish_media", "publish_jobs"] {
            let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})")).unwrap();
            let cols: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(1))
                .unwrap()
                .map(|c| c.unwrap().to_lowercase())
                .collect();
            for forbidden in ["token", "secret", "cookie", "password", "credential", "session"] {
                assert!(
                    !cols.iter().any(|c| c.contains(forbidden)),
                    "{table} gained a `{forbidden}`-ish column: {cols:?}"
                );
            }
        }
    }
}
