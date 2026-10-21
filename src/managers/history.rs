use crate::app_handle::AppHandle;
use anyhow::{anyhow, Result};
use chrono::{DateTime, Local, Utc};
use log::{debug, error, info};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use rusqlite_migration::{HookResult, Migrations, M};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Database migrations for transcription history.
/// Each migration is applied in order. The library tracks which migrations
/// have been applied using SQLite's user_version pragma.
///
/// Note: For users upgrading from tauri-plugin-sql, migrate_from_tauri_plugin_sql()
/// converts the old _sqlx_migrations table tracking to the user_version pragma,
/// ensuring migrations don't re-run on existing databases.
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
    M::up("ALTER TABLE transcription_history ADD COLUMN post_process_requested BOOLEAN NOT NULL DEFAULT 0;"),
    // Usage stats live in their own table so history retention cleanup doesn't erase them.
    M::up_with_hook(
        "CREATE TABLE usage_stats (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp INTEGER NOT NULL,
            word_count INTEGER NOT NULL,
            duration_ms INTEGER NOT NULL
        );",
        backfill_usage_stats,
    ),
    // Null for backfilled rows and platforms that can't detect the focused app.
    M::up("ALTER TABLE usage_stats ADD COLUMN app_name TEXT;"),
    // Annotate captures. Their own table keeps them out of the dictation streak.
    M::up(
        "CREATE TABLE annotate_usage (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp INTEGER NOT NULL,
            duration_ms INTEGER NOT NULL,
            mark_count INTEGER NOT NULL,
            app_name TEXT
        );",
    ),
    ])
}

/// Seed usage stats from existing history. Old entries have no recorded
/// duration, so they get 0 and are left out of the words-per-minute average.
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

/// Count words in a transcription. Chinese and Japanese are written without
/// spaces, so each ideograph or kana character counts as one word.
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
        '\u{3040}'..='\u{30FF}' // Hiragana, Katakana
        | '\u{3400}'..='\u{4DBF}' // CJK Extension A
        | '\u{4E00}'..='\u{9FFF}' // CJK Unified Ideographs
        | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaginatedHistory {
    pub entries: Vec<HistoryEntry>,
    pub has_more: bool,
}

/// Usage totals for one local calendar day. Days without dictation are omitted.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DailyUsage {
    /// YYYY-MM-DD in the user's local timezone.
    pub date: String,
    pub words: i64,
    pub transcriptions: i64,
    /// Words from transcriptions with a recorded duration. Entries backfilled
    /// from history have none, so words per minute uses this instead of `words`.
    pub timed_words: i64,
    pub duration_ms: i64,
    /// Distinct apps dictated into. 0 when no dictation recorded an app.
    pub apps_used: i64,
    /// App that received the most words, or None when no app was recorded.
    pub top_app: Option<String>,
}

/// One finished Annotate capture.
#[derive(Clone, Debug)]
pub struct CaptureUsage {
    /// Unix seconds when the recording started.
    pub timestamp: i64,
    pub duration_ms: i64,
    pub marks: i64,
    /// The app recorded. None for a whole display or an area.
    pub app_name: Option<String>,
}

/// Annotate totals for one local calendar day. Days without captures are omitted.
#[derive(Clone, Debug)]
pub struct DailyCaptures {
    /// YYYY-MM-DD in the user's local timezone.
    pub date: String,
    pub captures: i64,
    pub duration_ms: i64,
    pub marks: i64,
    /// App captured most often, or None when no capture recorded an app.
    pub top_app: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum HistoryUpdatePayload {
    #[serde(rename = "added")]
    Added { entry: HistoryEntry },
    #[serde(rename = "updated")]
    Updated { entry: HistoryEntry },
    #[serde(rename = "deleted")]
    Deleted { id: i64 },
    #[serde(rename = "toggled")]
    Toggled { id: i64 },
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
        // Create recordings directory in app data dir
        let app_data_dir = crate::portable::app_data_dir(app_handle)?;
        let recordings_dir = app_data_dir.join("recordings");
        let db_path = app_data_dir.join("history.db");

        // Ensure recordings directory exists
        if !recordings_dir.exists() {
            fs::create_dir_all(&recordings_dir)?;
            debug!("Created recordings directory: {:?}", recordings_dir);
        }

        let manager = Self {
            app_handle: app_handle.clone(),
            recordings_dir,
            db_path,
        };

        // Initialize database and run migrations synchronously
        manager.init_database()?;

        Ok(manager)
    }

    fn init_database(&self) -> Result<()> {
        info!("Initializing database at {:?}", self.db_path);

        let mut conn = Connection::open(&self.db_path)?;

        // Handle migration from tauri-plugin-sql to rusqlite_migration
        // tauri-plugin-sql used _sqlx_migrations table, rusqlite_migration uses user_version pragma
        self.migrate_from_tauri_plugin_sql(&conn)?;

        let migrations = migrations();

        // Validate migrations in debug builds
        #[cfg(debug_assertions)]
        migrations.validate().expect("Invalid migrations");

        // Get current version before migration
        let version_before: i32 =
            conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        debug!("Database version before migration: {}", version_before);

        // Apply any pending migrations
        migrations.to_latest(&mut conn)?;

        // Get version after migration
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
        // Check if the old _sqlx_migrations table exists
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

        // Check current user_version
        let current_version: i32 =
            conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

        if current_version > 0 {
            // Already migrated to rusqlite_migration system
            return Ok(());
        }

        // Get the highest version from the old migrations table
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

            // Set user_version to match the old migration state
            conn.pragma_update(None, "user_version", old_version)?;

            // Optionally drop the old migrations table (keeping it doesn't hurt)
            // conn.execute("DROP TABLE IF EXISTS _sqlx_migrations", [])?;

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

    /// Save a new history entry to the database.
    /// The WAV file should already have been written to the recordings directory.
    pub fn save_entry(
        &self,
        file_name: String,
        transcription_text: String,
        post_process_requested: bool,
        post_processed_text: Option<String>,
        post_process_prompt: Option<String>,
    ) -> Result<HistoryEntry> {
        let timestamp = Utc::now().timestamp();
        let title = self.format_timestamp_title(timestamp);

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

        // Emit typed event for real-time frontend updates
        if let Err(e) = (HistoryUpdatePayload::Added {
            entry: entry.clone(),
        })
        .emit(&self.app_handle)
        {
            error!("Failed to emit history-updated event: {}", e);
        }

        Ok(entry)
    }

    /// Update an existing history entry with new transcription results (used by retry).
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
            crate::settings::RecordingRetentionPeriod::Never => {
                // Don't delete anything
                Ok(())
            }
            crate::settings::RecordingRetentionPeriod::PreserveLimit => {
                // Use the old count-based logic with history_limit
                let limit = crate::settings::get_history_limit(&self.app_handle);
                self.cleanup_by_count(limit)
            }
            _ => {
                // Use time-based logic
                self.cleanup_by_time(retention_period)
            }
        }
    }

    fn delete_entries_and_files(&self, entries: &[(i64, String)]) -> Result<usize> {
        if entries.is_empty() {
            return Ok(0);
        }

        let conn = self.get_connection()?;
        let mut deleted_count = 0;

        for (id, file_name) in entries {
            // Delete database entry
            conn.execute(
                "DELETE FROM transcription_history WHERE id = ?1",
                params![id],
            )?;

            // Delete WAV file
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

        // Get all entries that are not saved, ordered by timestamp desc
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

        // Calculate cutoff timestamp (current time minus retention period)
        let now = Utc::now().timestamp();
        let cutoff_timestamp = match retention_period {
            crate::settings::RecordingRetentionPeriod::Days3 => now - (3 * 24 * 60 * 60), // 3 days in seconds
            crate::settings::RecordingRetentionPeriod::Weeks2 => now - (2 * 7 * 24 * 60 * 60), // 2 weeks in seconds
            crate::settings::RecordingRetentionPeriod::Months3 => now - (3 * 30 * 24 * 60 * 60), // 3 months in seconds (approximate)
            _ => unreachable!("Should not reach here"),
        };

        // Get all unsaved entries older than the cutoff timestamp
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
        limit: Option<usize>,
    ) -> Result<PaginatedHistory> {
        let conn = self.get_connection()?;
        let limit = limit.map(|l| l.min(100));

        let mut entries: Vec<HistoryEntry> = match (cursor, limit) {
            (Some(cursor_id), Some(lim)) => {
                let fetch_count = (lim + 1) as i64;
                let mut stmt = conn.prepare(
                    "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested
                     FROM transcription_history
                     WHERE id < ?1
                     ORDER BY id DESC
                     LIMIT ?2",
                )?;
                let result = stmt
                    .query_map(params![cursor_id, fetch_count], Self::map_history_entry)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                result
            }
            (None, Some(lim)) => {
                let fetch_count = (lim + 1) as i64;
                let mut stmt = conn.prepare(
                    "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested
                     FROM transcription_history
                     ORDER BY id DESC
                     LIMIT ?1",
                )?;
                let result = stmt
                    .query_map(params![fetch_count], Self::map_history_entry)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                result
            }
            (_, None) => {
                let mut stmt = conn.prepare(
                    "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested
                     FROM transcription_history
                     ORDER BY id DESC",
                )?;
                let result = stmt
                    .query_map([], Self::map_history_entry)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                result
            }
        };

        let has_more = limit.is_some_and(|lim| entries.len() > lim);
        if has_more {
            entries.pop();
        }

        Ok(PaginatedHistory { entries, has_more })
    }

    #[cfg(test)]
    fn get_latest_entry_with_conn(conn: &Connection) -> Result<Option<HistoryEntry>> {
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
             ORDER BY timestamp DESC
             LIMIT 1",
        )?;

        let entry = stmt.query_row([], Self::map_history_entry).optional()?;
        Ok(entry)
    }

    /// Get the latest entry with non-empty transcription text.
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

    /// Record one completed transcription for the stats page. `app_name` is
    /// the app the text is pasted into, when the platform can tell.
    pub fn record_usage(&self, text: &str, duration_ms: i64, app_name: Option<&str>) -> Result<()> {
        let conn = self.get_connection()?;
        Self::record_usage_with_conn(&conn, Utc::now().timestamp(), text, duration_ms, app_name)
    }

    fn record_usage_with_conn(
        conn: &Connection,
        timestamp: i64,
        text: &str,
        duration_ms: i64,
        app_name: Option<&str>,
    ) -> Result<()> {
        let word_count = count_words(text);
        if word_count == 0 {
            return Ok(());
        }
        conn.execute(
            "INSERT INTO usage_stats (timestamp, word_count, duration_ms, app_name)
             VALUES (?1, ?2, ?3, ?4)",
            params![timestamp, word_count, duration_ms, app_name],
        )?;
        Ok(())
    }

    /// Per-day usage, oldest first.
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

        // Each day's apps, most words first, so the first row per day is its top app.
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

    /// Record one finished Annotate capture for the stats page.
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

    /// Seed Annotate stats from the captures `load` finds on disk, while none
    /// are recorded yet: captures made before Bespoke counted them, or before
    /// an update added the table. Returns how many were recorded.
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

    /// Per-day Annotate captures, oldest first.
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

        // Each day's apps, most captures first, so the first row per day is its top app.
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

        // Get current saved status
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

        // Emit history updated event
        if let Err(e) = (HistoryUpdatePayload::Toggled { id }).emit(&self.app_handle) {
            error!("Failed to emit history-updated event: {}", e);
        }

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

        // Get the entry to find the file name
        if let Some(entry) = self.get_entry_by_id(id).await? {
            // Delete the audio file first
            let file_path = self.get_audio_file_path(&entry.file_name);
            if file_path.exists() {
                if let Err(e) = fs::remove_file(&file_path) {
                    error!("Failed to delete audio file {}: {}", entry.file_name, e);
                    // Continue with database deletion even if file deletion fails
                }
            }
        }

        // Delete from database
        conn.execute(
            "DELETE FROM transcription_history WHERE id = ?1",
            params![id],
        )?;

        debug!("Deleted history entry with id: {}", id);

        // Emit history updated event
        if let Err(e) = (HistoryUpdatePayload::Deleted { id }).emit(&self.app_handle) {
            error!("Failed to emit history-updated event: {}", e);
        }

        Ok(())
    }

    fn format_timestamp_title(&self, timestamp: i64) -> String {
        if let Some(utc_datetime) = DateTime::from_timestamp(timestamp, 0) {
            // Convert UTC to local timezone
            let local_datetime = utc_datetime.with_timezone(&Local);
            local_datetime.format("%B %e, %Y - %l:%M%p").to_string()
        } else {
            format!("Recording {}", timestamp)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{params, Connection};

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
        // Only the recorded dictation has a duration.
        assert_eq!(sum(|d| d.timed_words), 4);
        assert_eq!(sum(|d| d.duration_ms), 2_000);
    }

    #[test]
    fn daily_usage_counts_apps_and_picks_top_app_by_words() {
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        migrations()
            .to_latest(&mut conn)
            .expect("migrate to latest");
        let day = 1_788_264_000; // Noon UTC, so every timezone sees the same date.
        let next_day = day + 2 * 86_400;
        let record = |timestamp, text, app| {
            HistoryManager::record_usage_with_conn(&conn, timestamp, text, 1_000, app)
                .expect("record usage");
        };
        record(day, "one two", Some("Slack"));
        record(day + 60, "one two three", Some("Codex"));
        record(day + 120, "one two", Some("Slack"));
        record(day + 180, "one two three four five", None);
        record(next_day, "one two three", None);

        let days = HistoryManager::get_daily_usage_with_conn(&conn).expect("daily usage");
        assert_eq!(days.len(), 2);
        // Slack's 4 words beat Codex's 3; the untracked dictation counts toward neither.
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
        let day = 1_788_264_000; // Noon UTC, so every timezone sees the same date.
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
    fn get_latest_entry_returns_none_when_empty() {
        let conn = setup_conn();
        let entry = HistoryManager::get_latest_entry_with_conn(&conn).expect("fetch latest entry");
        assert!(entry.is_none());
    }

    #[test]
    fn get_latest_entry_returns_newest_entry() {
        let conn = setup_conn();
        insert_entry(&conn, 100, "first", None);
        insert_entry(&conn, 200, "second", Some("processed"));

        let entry = HistoryManager::get_latest_entry_with_conn(&conn)
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
