use crate::app_handle::AppHandle;
use anyhow::{Result, anyhow};
use chrono::{DateTime, Local, Utc};
use log::{debug, error, info};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use rusqlite_migration::{HookResult, M, Migrations};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(
            "CREATE TABLE IF NOT EXISTS transcription_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_name TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            saved BOOLEAN NOT NULL DEFAULT 0,
            title TEXT NOT NULL,
            transcription_text TEXT NOT NULL
        );",
        ),
        M::up("ALTER TABLE transcription_history ADD COLUMN post_processed_text TEXT;"),
        M::up("ALTER TABLE transcription_history ADD COLUMN post_process_prompt TEXT;"),
        M::up(
            "ALTER TABLE transcription_history ADD COLUMN post_process_requested BOOLEAN NOT NULL DEFAULT 0;",
        ),
        M::up_with_hook(
            "CREATE TABLE usage_stats (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp INTEGER NOT NULL,
            word_count INTEGER NOT NULL,
            duration_ms INTEGER NOT NULL
        );",
            backfill_usage_stats,
        ),
        M::up("ALTER TABLE usage_stats ADD COLUMN app_name TEXT;"),
        M::up(
            "CREATE TABLE annotate_usage (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp INTEGER NOT NULL,
            duration_ms INTEGER NOT NULL,
            mark_count INTEGER NOT NULL,
            app_name TEXT
        );",
        ),
        M::up("ALTER TABLE usage_stats ADD COLUMN bundle_id TEXT;"),
    ])
}

fn backfill_usage_stats(tx: &Transaction) -> HookResult {
    let mut stmt = tx.prepare(
        "SELECT timestamp, transcription_text FROM transcription_history WHERE transcription_text != ''",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (timestamp, text) in rows {
        let word_count = count_words(&text);
        if word_count > 0 {
            tx.execute(
                "INSERT INTO usage_stats (timestamp, word_count, duration_ms) VALUES (?1, ?2, 0)",
                params![timestamp, word_count],
            )?;
        }
    }
    Ok(())
}

fn count_words(text: &str) -> i64 {
    let mut count = 0;
    let mut in_word = false;
    for c in text.chars() {
        if is_cjk(c) {
            count += 1;
            in_word = false;
        } else if c.is_alphanumeric() {
            if !in_word {
                count += 1;
                in_word = true;
            }
        } else if c.is_whitespace() {
            in_word = false;
        }
    }
    count
}

fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3040}'..='\u{30FF}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{4E00}'..='\u{9FFF}'
        | '\u{F900}'..='\u{FAFF}'
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaginatedHistory {
    pub entries: Vec<HistoryEntry>,
    pub has_more: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PasteTarget {
    pub name: Option<String>,
    pub bundle_id: Option<String>,
}

impl PasteTarget {
    /// Reads the name and bundle id from one lookup, so both describe the same app.
    pub fn frontmost() -> Self {
        let Some(app) = objc2_app_kit::NSWorkspace::sharedWorkspace().frontmostApplication() else {
            return Self::default();
        };
        Self {
            name: app.localizedName().map(|name| name.to_string()),
            bundle_id: app.bundleIdentifier().map(|id| id.to_string()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DailyUsage {
    pub date: String,
    pub words: i64,
    pub transcriptions: i64,
    pub timed_words: i64,
    pub duration_ms: i64,
    pub apps_used: i64,
    pub top_app: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CaptureUsage {
    pub timestamp: i64,
    pub duration_ms: i64,
    pub marks: i64,
    pub app_name: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DailyCaptures {
    pub date: String,
    pub captures: i64,
    pub duration_ms: i64,
    pub marks: i64,
    pub top_app: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum HistoryUpdatePayload {
    #[serde(rename = "added")]
    Added { entry: HistoryEntry },
    #[serde(rename = "updated")]
    Updated { entry: HistoryEntry },
}

impl HistoryUpdatePayload {
    pub const EVENT: &'static str = "history-update-payload";

    pub fn emit(&self, app: &AppHandle) -> Result<(), crate::app_handle::Error> {
        app.emit(Self::EVENT, self)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: i64,
    pub file_name: String,
    pub timestamp: i64,
    pub saved: bool,
    pub title: String,
    pub transcription_text: String,
    pub post_processed_text: Option<String>,
    pub post_process_prompt: Option<String>,
    pub post_process_requested: bool,
}

pub struct HistoryManager {
    app_handle: AppHandle,
    recordings_dir: PathBuf,
    db_path: PathBuf,
}

impl HistoryManager {
    pub fn new(app_handle: &AppHandle) -> Result<Self> {
        let app_data_dir = crate::portable::app_data_dir(app_handle)?;
        let recordings_dir = app_data_dir.join("recordings");
        let db_path = app_data_dir.join("history.db");

        if !recordings_dir.exists() {
            fs::create_dir_all(&recordings_dir)?;
            debug!("Created recordings directory: {:?}", recordings_dir);
        }

        let manager = Self {
            app_handle: app_handle.clone(),
            recordings_dir,
            db_path,
        };

        manager.init_database()?;

        Ok(manager)
    }

    fn init_database(&self) -> Result<()> {
        info!("Initializing database at {:?}", self.db_path);

        let mut conn = Connection::open(&self.db_path)?;

        self.migrate_from_tauri_plugin_sql(&conn)?;

        let migrations = migrations();

        #[cfg(debug_assertions)]
        migrations.validate().expect("Invalid migrations");

        let version_before: i32 =
            conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        debug!("Database version before migration: {}", version_before);

        migrations.to_latest(&mut conn)?;

        let version_after: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

        if version_after > version_before {
            info!(
                "Database migrated from version {} to {}",
                version_before, version_after
            );
        } else {
            debug!("Database already at latest version {}", version_after);
        }

        Ok(())
    }

    /// Migrate from tauri-plugin-sql's migration tracking to rusqlite_migration's.
    /// tauri-plugin-sql used a _sqlx_migrations table, while rusqlite_migration uses
    /// SQLite's user_version pragma. This function checks if the old system was in use
    /// and sets the user_version accordingly so migrations don't re-run.
    fn migrate_from_tauri_plugin_sql(&self, conn: &Connection) -> Result<()> {
        let has_sqlx_migrations: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !has_sqlx_migrations {
            return Ok(());
        }

        let current_version: i32 =
            conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

        if current_version > 0 {
            return Ok(());
        }

        let old_version: i32 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations WHERE success = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if old_version > 0 {
            info!(
                "Migrating from tauri-plugin-sql (version {}) to rusqlite_migration",
                old_version
            );

            conn.pragma_update(None, "user_version", old_version)?;

            info!(
                "Migration tracking converted: user_version set to {}",
                old_version
            );
        }

        Ok(())
    }

    fn get_connection(&self) -> Result<Connection> {
        Ok(Connection::open(&self.db_path)?)
    }

    fn map_history_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryEntry> {
        Ok(HistoryEntry {
            id: row.get("id")?,
            file_name: row.get("file_name")?,
            timestamp: row.get("timestamp")?,
            saved: row.get("saved")?,
            title: row.get("title")?,
            transcription_text: row.get("transcription_text")?,
            post_processed_text: row.get("post_processed_text")?,
            post_process_prompt: row.get("post_process_prompt")?,
            post_process_requested: row.get("post_process_requested")?,
        })
    }

    pub fn recordings_dir(&self) -> &std::path::Path {
        &self.recordings_dir
    }

    pub fn save_entry(
        &self,
        file_name: String,
        transcription_text: String,
        post_process_requested: bool,
        post_processed_text: Option<String>,
        post_process_prompt: Option<String>,
    ) -> Result<HistoryEntry> {
        let timestamp = Utc::now().timestamp();
        let title = format_timestamp_title(timestamp);

        let conn = self.get_connection()?;
        conn.execute(
            "INSERT INTO transcription_history (
                file_name,
                timestamp,
                saved,
                title,
                transcription_text,
                post_processed_text,
                post_process_prompt,
                post_process_requested
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                &file_name,
                timestamp,
                false,
                &title,
                &transcription_text,
                &post_processed_text,
                &post_process_prompt,
                post_process_requested,
            ],
        )?;

        let entry = HistoryEntry {
            id: conn.last_insert_rowid(),
            file_name,
            timestamp,
            saved: false,
            title,
            transcription_text,
            post_processed_text,
            post_process_prompt,
            post_process_requested,
        };

        debug!("Saved history entry with id {}", entry.id);

        self.cleanup_old_entries()?;

        if let Err(e) = (HistoryUpdatePayload::Added {
            entry: entry.clone(),
        })
        .emit(&self.app_handle)
        {
            error!("Failed to emit history-updated event: {}", e);
        }

        Ok(entry)
    }

    pub fn update_transcription(
        &self,
        id: i64,
        transcription_text: String,
        post_processed_text: Option<String>,
        post_process_prompt: Option<String>,
    ) -> Result<HistoryEntry> {
        let conn = self.get_connection()?;
        let updated = conn.execute(
            "UPDATE transcription_history
             SET transcription_text = ?1,
                 post_processed_text = ?2,
                 post_process_prompt = ?3
             WHERE id = ?4",
            params![
                transcription_text,
                post_processed_text,
                post_process_prompt,
                id
            ],
        )?;

        if updated == 0 {
            return Err(anyhow!("History entry {} not found", id));
        }

        let entry = conn
            .query_row(
                "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested
                 FROM transcription_history WHERE id = ?1",
                params![id],
                Self::map_history_entry,
            )?;

        debug!("Updated transcription for history entry {}", id);

        if let Err(e) = (HistoryUpdatePayload::Updated {
            entry: entry.clone(),
        })
        .emit(&self.app_handle)
        {
            error!("Failed to emit history-updated event: {}", e);
        }

        Ok(entry)
    }

    pub fn cleanup_old_entries(&self) -> Result<()> {
        let retention_period = crate::settings::get_recording_retention_period(&self.app_handle);

        match retention_period {
            crate::settings::RecordingRetentionPeriod::Never => Ok(()),
            crate::settings::RecordingRetentionPeriod::PreserveLimit => {
                let limit = crate::settings::get_history_limit(&self.app_handle);
                self.cleanup_by_count(limit)
            }
            _ => self.cleanup_by_time(retention_period),
        }
    }

    fn delete_entries_and_files(&self, entries: &[(i64, String)]) -> Result<usize> {
        if entries.is_empty() {
            return Ok(0);
        }

        let conn = self.get_connection()?;
        let mut deleted_count = 0;

        for (id, file_name) in entries {
            conn.execute(
                "DELETE FROM transcription_history WHERE id = ?1",
                params![id],
            )?;

            let file_path = self.recordings_dir.join(file_name);
            if file_path.exists() {
                if let Err(e) = fs::remove_file(&file_path) {
                    error!("Failed to delete WAV file {}: {}", file_name, e);
                } else {
                    debug!("Deleted old WAV file: {}", file_name);
                    deleted_count += 1;
                }
            }
        }

        Ok(deleted_count)
    }

    fn cleanup_by_count(&self, limit: usize) -> Result<()> {
        let conn = self.get_connection()?;

        let mut stmt = conn.prepare(
            "SELECT id, file_name FROM transcription_history WHERE saved = 0 ORDER BY timestamp DESC"
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>("id")?, row.get::<_, String>("file_name")?))
        })?;

        let mut entries: Vec<(i64, String)> = Vec::new();
        for row in rows {
            entries.push(row?);
        }

        if entries.len() > limit {
            let entries_to_delete = &entries[limit..];
            let deleted_count = self.delete_entries_and_files(entries_to_delete)?;

            if deleted_count > 0 {
                debug!("Cleaned up {} old history entries by count", deleted_count);
            }
        }

        Ok(())
    }

    fn cleanup_by_time(
        &self,
        retention_period: crate::settings::RecordingRetentionPeriod,
    ) -> Result<()> {
        let conn = self.get_connection()?;

        let now = Utc::now().timestamp();
        let cutoff_timestamp = match retention_period {
            crate::settings::RecordingRetentionPeriod::Days3 => now - (3 * 24 * 60 * 60),
            crate::settings::RecordingRetentionPeriod::Weeks2 => now - (2 * 7 * 24 * 60 * 60),
            crate::settings::RecordingRetentionPeriod::Months3 => now - (3 * 30 * 24 * 60 * 60),
            _ => unreachable!("Should not reach here"),
        };

        let mut stmt = conn.prepare(
            "SELECT id, file_name FROM transcription_history WHERE saved = 0 AND timestamp < ?1",
        )?;

        let rows = stmt.query_map(params![cutoff_timestamp], |row| {
            Ok((row.get::<_, i64>("id")?, row.get::<_, String>("file_name")?))
        })?;

        let mut entries_to_delete: Vec<(i64, String)> = Vec::new();
        for row in rows {
            entries_to_delete.push(row?);
        }

        let deleted_count = self.delete_entries_and_files(&entries_to_delete)?;

        if deleted_count > 0 {
            debug!(
                "Cleaned up {} old history entries based on retention period",
                deleted_count
            );
        }

        Ok(())
    }

    pub async fn get_history_entries(
        &self,
        cursor: Option<i64>,
        limit: usize,
    ) -> Result<PaginatedHistory> {
        let conn = self.get_connection()?;
        let limit = limit.min(100);
        let fetch_count = (limit + 1) as i64;

        let mut entries: Vec<HistoryEntry> = match cursor {
            Some(cursor_id) => {
                let mut stmt = conn.prepare(
                    "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested
                     FROM transcription_history
                     WHERE id < ?1
                     ORDER BY id DESC
                     LIMIT ?2",
                )?;

                stmt.query_map(params![cursor_id, fetch_count], Self::map_history_entry)?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            }
            None => {
                let mut stmt = conn.prepare(
                    "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested
                     FROM transcription_history
                     ORDER BY id DESC
                     LIMIT ?1",
                )?;

                stmt.query_map(params![fetch_count], Self::map_history_entry)?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            }
        };

        let has_more = entries.len() > limit;
        if has_more {
            entries.pop();
        }

        Ok(PaginatedHistory { entries, has_more })
    }

    pub fn get_latest_completed_entry(&self) -> Result<Option<HistoryEntry>> {
        let conn = self.get_connection()?;
        Self::get_latest_completed_entry_with_conn(&conn)
    }

    fn get_latest_completed_entry_with_conn(conn: &Connection) -> Result<Option<HistoryEntry>> {
        let mut stmt = conn.prepare(
            "SELECT
                id,
                file_name,
                timestamp,
                saved,
                title,
                transcription_text,
                post_processed_text,
                post_process_prompt,
                post_process_requested
             FROM transcription_history
             WHERE transcription_text != ''
             ORDER BY timestamp DESC
             LIMIT 1",
        )?;

        let entry = stmt.query_row([], Self::map_history_entry).optional()?;
        Ok(entry)
    }

    pub fn record_usage(
        &self,
        text: &str,
        duration_ms: i64,
        app: Option<&PasteTarget>,
    ) -> Result<()> {
        let conn = self.get_connection()?;
        Self::record_usage_with_conn(&conn, Utc::now().timestamp(), text, duration_ms, app)
    }

    fn record_usage_with_conn(
        conn: &Connection,
        timestamp: i64,
        text: &str,
        duration_ms: i64,
        app: Option<&PasteTarget>,
    ) -> Result<()> {
        let word_count = count_words(text);
        if word_count == 0 {
            return Ok(());
        }
        conn.execute(
            "INSERT INTO usage_stats (timestamp, word_count, duration_ms, app_name, bundle_id)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                timestamp,
                word_count,
                duration_ms,
                app.and_then(|app| app.name.as_deref()),
                app.and_then(|app| app.bundle_id.as_deref()),
            ],
        )?;
        Ok(())
    }

    pub fn get_daily_usage(&self) -> Result<Vec<DailyUsage>> {
        let conn = self.get_connection()?;
        Self::get_daily_usage_with_conn(&conn)
    }

    fn get_daily_usage_with_conn(conn: &Connection) -> Result<Vec<DailyUsage>> {
        let mut stmt = conn.prepare(
            "SELECT
                date(timestamp, 'unixepoch', 'localtime') AS day,
                SUM(word_count),
                COUNT(*),
                SUM(CASE WHEN duration_ms > 0 THEN word_count ELSE 0 END),
                SUM(duration_ms),
                COUNT(DISTINCT app_name)
             FROM usage_stats
             GROUP BY day
             ORDER BY day",
        )?;
        let mut days = stmt
            .query_map([], |row| {
                Ok(DailyUsage {
                    date: row.get(0)?,
                    words: row.get(1)?,
                    transcriptions: row.get(2)?,
                    timed_words: row.get(3)?,
                    duration_ms: row.get(4)?,
                    apps_used: row.get(5)?,
                    top_app: None,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut stmt = conn.prepare(
            "SELECT
                date(timestamp, 'unixepoch', 'localtime') AS day,
                app_name
             FROM usage_stats
             WHERE app_name IS NOT NULL
             GROUP BY day, app_name
             ORDER BY day, SUM(word_count) DESC, app_name",
        )?;
        let mut top_apps: HashMap<String, String> = HashMap::new();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            top_apps.entry(row.get(0)?).or_insert(row.get(1)?);
        }
        for day in &mut days {
            day.top_app = top_apps.remove(&day.date);
        }
        Ok(days)
    }

    pub fn record_capture(&self, capture: &CaptureUsage) -> Result<()> {
        let conn = self.get_connection()?;
        Self::record_capture_with_conn(&conn, capture)
    }

    fn record_capture_with_conn(conn: &Connection, capture: &CaptureUsage) -> Result<()> {
        conn.execute(
            "INSERT INTO annotate_usage (timestamp, duration_ms, mark_count, app_name)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                capture.timestamp,
                capture.duration_ms,
                capture.marks,
                capture.app_name
            ],
        )?;
        Ok(())
    }

    pub fn seed_captures(&self, load: impl FnOnce() -> Vec<CaptureUsage>) -> Result<usize> {
        let mut conn = self.get_connection()?;
        Self::seed_captures_with_conn(&mut conn, load)
    }

    fn seed_captures_with_conn(
        conn: &mut Connection,
        load: impl FnOnce() -> Vec<CaptureUsage>,
    ) -> Result<usize> {
        let recorded: bool =
            conn.query_row("SELECT EXISTS (SELECT 1 FROM annotate_usage)", [], |row| {
                row.get(0)
            })?;
        if recorded {
            return Ok(0);
        }
        let captures = load();
        let tx = conn.transaction()?;
        for capture in &captures {
            Self::record_capture_with_conn(&tx, capture)?;
        }
        tx.commit()?;
        Ok(captures.len())
    }

    pub fn get_daily_captures(&self) -> Result<Vec<DailyCaptures>> {
        let conn = self.get_connection()?;
        Self::get_daily_captures_with_conn(&conn)
    }

    fn get_daily_captures_with_conn(conn: &Connection) -> Result<Vec<DailyCaptures>> {
        let mut stmt = conn.prepare(
            "SELECT
                date(timestamp, 'unixepoch', 'localtime') AS day,
                COUNT(*),
                SUM(duration_ms),
                SUM(mark_count)
             FROM annotate_usage
             GROUP BY day
             ORDER BY day",
        )?;
        let mut days = stmt
            .query_map([], |row| {
                Ok(DailyCaptures {
                    date: row.get(0)?,
                    captures: row.get(1)?,
                    duration_ms: row.get(2)?,
                    marks: row.get(3)?,
                    top_app: None,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut stmt = conn.prepare(
            "SELECT
                date(timestamp, 'unixepoch', 'localtime') AS day,
                app_name
             FROM annotate_usage
             WHERE app_name IS NOT NULL
             GROUP BY day, app_name
             ORDER BY day, COUNT(*) DESC, app_name",
        )?;
        let mut top_apps: HashMap<String, String> = HashMap::new();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            top_apps.entry(row.get(0)?).or_insert(row.get(1)?);
        }
        for day in &mut days {
            day.top_app = top_apps.remove(&day.date);
        }
        Ok(days)
    }

    pub async fn toggle_saved_status(&self, id: i64) -> Result<()> {
        let conn = self.get_connection()?;

        let current_saved: bool = conn.query_row(
            "SELECT saved FROM transcription_history WHERE id = ?1",
            params![id],
            |row| row.get("saved"),
        )?;

        let new_saved = !current_saved;

        conn.execute(
            "UPDATE transcription_history SET saved = ?1 WHERE id = ?2",
            params![new_saved, id],
        )?;

        debug!("Toggled saved status for entry {}: {}", id, new_saved);

        Ok(())
    }

    pub fn get_audio_file_path(&self, file_name: &str) -> PathBuf {
        self.recordings_dir.join(file_name)
    }

    pub async fn get_entry_by_id(&self, id: i64) -> Result<Option<HistoryEntry>> {
        let conn = self.get_connection()?;
        let mut stmt = conn.prepare(
            "SELECT
                id,
                file_name,
                timestamp,
                saved,
                title,
                transcription_text,
                post_processed_text,
                post_process_prompt,
                post_process_requested
             FROM transcription_history
             WHERE id = ?1",
        )?;

        let entry = stmt.query_row([id], Self::map_history_entry).optional()?;

        Ok(entry)
    }

    pub async fn delete_entry(&self, id: i64) -> Result<()> {
        let conn = self.get_connection()?;

        if let Some(entry) = self.get_entry_by_id(id).await? {
            let file_path = self.get_audio_file_path(&entry.file_name);
            if file_path.exists()
                && let Err(e) = fs::remove_file(&file_path)
            {
                error!("Failed to delete audio file {}: {}", entry.file_name, e);
            }
        }

        conn.execute(
            "DELETE FROM transcription_history WHERE id = ?1",
            params![id],
        )?;

        debug!("Deleted history entry with id: {}", id);

        Ok(())
    }
}

fn format_timestamp_title(timestamp: i64) -> String {
    if let Some(utc_datetime) = DateTime::from_timestamp(timestamp, 0) {
        let local_datetime = utc_datetime.with_timezone(&Local);
        local_datetime.format("%B %e, %Y - %l:%M%p").to_string()
    } else {
        format!("Recording {}", timestamp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{Connection, params};

    fn setup_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch(
            "CREATE TABLE transcription_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_name TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                saved BOOLEAN NOT NULL DEFAULT 0,
                title TEXT NOT NULL,
                transcription_text TEXT NOT NULL,
                post_processed_text TEXT,
                post_process_prompt TEXT,
                post_process_requested BOOLEAN NOT NULL DEFAULT 0
            );",
        )
        .expect("create transcription_history table");
        conn
    }

    fn insert_entry(conn: &Connection, timestamp: i64, text: &str, post_processed: Option<&str>) {
        conn.execute(
            "INSERT INTO transcription_history (
                file_name,
                timestamp,
                saved,
                title,
                transcription_text,
                post_processed_text,
                post_process_prompt,
                post_process_requested
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                format!("bespoke-{}.wav", timestamp),
                timestamp,
                false,
                format!("Recording {}", timestamp),
                text,
                post_processed,
                Option::<String>::None,
                false,
            ],
        )
        .expect("insert history entry");
    }

    #[test]
    fn count_words_handles_punctuation_and_cjk() {
        assert_eq!(count_words(""), 0);
        assert_eq!(count_words("  ...  "), 0);
        assert_eq!(count_words("Hello, world! It's well-known."), 4);
        assert_eq!(count_words("你好世界"), 4);
        assert_eq!(count_words("Bespoke は すごい"), 5);
    }

    #[test]
    fn usage_stats_migration_backfills_history_without_duration() {
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        let migrations = migrations();
        migrations.to_version(&mut conn, 4).expect("migrate to v4");
        insert_entry(&conn, 100, "one two three", None);
        insert_entry(&conn, 200, "", None);
        migrations.to_latest(&mut conn).expect("migrate to latest");

        HistoryManager::record_usage_with_conn(&conn, 300, "four five six seven", 2_000, None)
            .expect("record usage");
        HistoryManager::record_usage_with_conn(&conn, 400, "  ", 1_000, None)
            .expect("record usage");

        let days = HistoryManager::get_daily_usage_with_conn(&conn).expect("daily usage");
        let sum = |field: fn(&DailyUsage) -> i64| days.iter().map(field).sum::<i64>();
        assert_eq!(sum(|d| d.words), 7);
        assert_eq!(sum(|d| d.transcriptions), 2);
        assert_eq!(sum(|d| d.timed_words), 4);
        assert_eq!(sum(|d| d.duration_ms), 2_000);
    }

    #[test]
    fn daily_usage_counts_apps_and_picks_top_app_by_words() {
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        migrations()
            .to_latest(&mut conn)
            .expect("migrate to latest");
        let day = 1_788_264_000;
        let next_day = day + 2 * 86_400;
        let record = |timestamp, text, app: Option<&str>| {
            let app = app.map(|name| PasteTarget {
                name: Some(name.to_string()),
                bundle_id: None,
            });
            HistoryManager::record_usage_with_conn(&conn, timestamp, text, 1_000, app.as_ref())
                .expect("record usage");
        };
        record(day, "one two", Some("Slack"));
        record(day + 60, "one two three", Some("Codex"));
        record(day + 120, "one two", Some("Slack"));
        record(day + 180, "one two three four five", None);
        record(next_day, "one two three", None);

        let days = HistoryManager::get_daily_usage_with_conn(&conn).expect("daily usage");
        assert_eq!(days.len(), 2);
        assert_eq!(days[0].apps_used, 2);
        assert_eq!(days[0].top_app.as_deref(), Some("Slack"));
        assert_eq!(days[1].apps_used, 0);
        assert_eq!(days[1].top_app, None);
    }

    #[test]
    fn daily_captures_sum_each_day_and_pick_top_app_by_count() {
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        migrations()
            .to_latest(&mut conn)
            .expect("migrate to latest");
        let day = 1_788_264_000;
        let record = |timestamp, duration_ms, marks, app: Option<&str>| {
            let capture = CaptureUsage {
                timestamp,
                duration_ms,
                marks,
                app_name: app.map(str::to_string),
            };
            HistoryManager::record_capture_with_conn(&conn, &capture).expect("record capture");
        };
        record(day, 30_000, 2, Some("Slack"));
        record(day + 60, 10_000, 0, Some("Ghostty"));
        record(day + 120, 20_000, 3, Some("Slack"));
        record(day + 2 * 86_400, 5_000, 1, None);

        let days = HistoryManager::get_daily_captures_with_conn(&conn).expect("daily captures");
        assert_eq!(days.len(), 2);
        assert_eq!(
            (days[0].captures, days[0].duration_ms, days[0].marks),
            (3, 60_000, 5)
        );
        assert_eq!(days[0].top_app.as_deref(), Some("Slack"));
        assert_eq!(days[1].top_app, None);
    }

    #[test]
    fn seed_captures_loads_only_while_none_are_recorded() {
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        migrations()
            .to_latest(&mut conn)
            .expect("migrate to latest");
        let capture = CaptureUsage {
            timestamp: 1_788_264_000,
            duration_ms: 1_000,
            marks: 1,
            app_name: None,
        };

        let seeded =
            HistoryManager::seed_captures_with_conn(&mut conn, || vec![capture.clone(); 2])
                .expect("seed captures");
        assert_eq!(seeded, 2);
        let seeded = HistoryManager::seed_captures_with_conn(&mut conn, || {
            unreachable!("the table already has captures")
        })
        .expect("seed captures");
        assert_eq!(seeded, 0);
    }

    #[test]
    fn get_latest_completed_entry_returns_none_when_empty() {
        let conn = setup_conn();
        let entry = HistoryManager::get_latest_completed_entry_with_conn(&conn)
            .expect("fetch latest entry");
        assert!(entry.is_none());
    }

    #[test]
    fn get_latest_completed_entry_returns_newest_entry() {
        let conn = setup_conn();
        insert_entry(&conn, 100, "first", None);
        insert_entry(&conn, 200, "second", Some("processed"));

        let entry = HistoryManager::get_latest_completed_entry_with_conn(&conn)
            .expect("fetch latest entry")
            .expect("entry exists");

        assert_eq!(entry.timestamp, 200);
        assert_eq!(entry.transcription_text, "second");
        assert_eq!(entry.post_processed_text.as_deref(), Some("processed"));
    }

    #[test]
    fn get_latest_completed_entry_skips_empty_entries() {
        let conn = setup_conn();
        insert_entry(&conn, 100, "completed", None);
        insert_entry(&conn, 200, "", None);

        let entry = HistoryManager::get_latest_completed_entry_with_conn(&conn)
            .expect("fetch latest completed entry")
            .expect("completed entry exists");

        assert_eq!(entry.timestamp, 100);
        assert_eq!(entry.transcription_text, "completed");
    }
}
