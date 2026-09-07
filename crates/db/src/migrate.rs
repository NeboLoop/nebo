use rusqlite::Connection;
use rust_embed::Embed;
use tracing::info;

use types::NeboError;

/// Embedded SQL migration files.
#[derive(Embed)]
#[folder = "migrations/"]
#[include = "*.sql"]
struct Migrations;

use rust_embed::EmbeddedFile;

// Helper to use rust-embed v8 trait methods
fn iter_files() -> Vec<String> {
    <Migrations as Embed>::iter()
        .map(|f| f.to_string())
        .collect()
}

fn get_file(name: &str) -> Option<EmbeddedFile> {
    <Migrations as Embed>::get(name)
}

/// Run all pending migrations on the database connection.
/// Compatible with goose's migration tracking (goose_db_version table).
pub fn run_migrations(conn: &Connection) -> Result<(), NeboError> {
    run_migrations_to(conn, i64::MAX)
}

/// Apply every pending migration up to and including `max_version`. The
/// proof builds a database in an older shape this way, then upgrades it.
pub fn run_migrations_to(conn: &Connection, max_version: i64) -> Result<(), NeboError> {
    // Create our migration tracking table if it doesn't exist
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _nebo_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )
    .map_err(|e| NeboError::Migration(format!("failed to create migrations table: {e}")))?;

    // Check for goose's table and reconcile if needed
    reconcile_goose_versions(conn)?;

    // Get list of already-applied migrations
    let applied: Vec<i64> = {
        let mut stmt = conn
            .prepare("SELECT version FROM _nebo_migrations ORDER BY version")
            .map_err(|e| NeboError::Migration(format!("failed to query migrations: {e}")))?;
        stmt.query_map([], |row| row.get(0))
            .map_err(|e| NeboError::Migration(format!("failed to read migrations: {e}")))?
            .filter_map(|r| r.ok())
            .collect()
    };

    // Get all migration files sorted by name
    let mut migration_files = iter_files();
    migration_files.sort();

    // A versioned copy of the database before anything changes, taken by
    // SQLite itself (consistent, online). Rollback is the previous binary
    // plus this file. Only for a database that has history: a fresh one has
    // nothing to lose. One file per starting version, overwritten if the
    // same upgrade is attempted again.
    let pending: Vec<&String> = migration_files
        .iter()
        .filter(|f| extract_version(f).map(|v| v <= max_version && !applied.contains(&v)).unwrap_or(false))
        .collect();
    if !pending.is_empty() && !applied.is_empty() {
        if let Some(path) = conn.path().filter(|p| !p.is_empty() && *p != ":memory:") {
            let from = applied.iter().max().copied().unwrap_or(0);
            let backup = format!("{path}.pre-v{from:04}.bak");
            let _ = std::fs::remove_file(&backup);
            match conn.execute("VACUUM INTO ?1", rusqlite::params![backup]) {
                Ok(_) => info!(backup = %backup, from_version = from, pending = pending.len(), "pre-migration database copy written"),
                Err(e) => {
                    return Err(NeboError::Migration(format!(
                        "refusing to migrate without a pre-migration copy ({backup}): {e}"
                    )));
                }
            }
        }
    }

    let mut applied_count = 0;

    for filename in &migration_files {
        // Extract version number from filename (e.g., "0001_initial_schema.sql" -> 1)
        let version = extract_version(filename).ok_or_else(|| {
            NeboError::Migration(format!("invalid migration filename: {filename}"))
        })?;

        if applied.contains(&version) || version > max_version {
            continue;
        }

        // Read migration SQL
        let data = get_file(filename)
            .ok_or_else(|| NeboError::Migration(format!("migration file not found: {filename}")))?;
        let sql = String::from_utf8_lossy(&data.data);

        // Extract only the "Up" portion (skip goose Down sections)
        let up_sql = extract_goose_up(&sql);

        info!(version, filename, "applying migration");

        // Execute migration in a transaction
        conn.execute_batch("BEGIN;")
            .map_err(|e| NeboError::Migration(format!("failed to begin transaction: {e}")))?;

        match conn.execute_batch(&up_sql) {
            Ok(()) => {
                conn.execute(
                    "INSERT INTO _nebo_migrations (version, name) VALUES (?1, ?2)",
                    rusqlite::params![version, filename],
                )
                .map_err(|e| {
                    let _ = conn.execute_batch("ROLLBACK;");
                    NeboError::Migration(format!("failed to record migration {filename}: {e}"))
                })?;
                conn.execute_batch("COMMIT;")
                    .map_err(|e| NeboError::Migration(format!("failed to commit: {e}")))?;
                applied_count += 1;
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(NeboError::Migration(format!(
                    "migration {filename} failed: {e}"
                )));
            }
        }
    }

    if applied_count > 0 {
        info!(applied_count, "migrations applied successfully");
    } else {
        info!("database is up to date");
    }

    Ok(())
}

/// If goose's `goose_db_version` table exists, import its versions into our tracker.
fn reconcile_goose_versions(conn: &Connection) -> Result<(), NeboError> {
    // Check if goose table exists
    let has_goose: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='goose_db_version'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if !has_goose {
        return Ok(());
    }

    info!("detected goose migration table, reconciling...");

    // Import goose versions we haven't tracked yet
    conn.execute_batch(
        "INSERT OR IGNORE INTO _nebo_migrations (version, name)
         SELECT version_id, CAST(version_id AS TEXT) || '_goose_imported.sql'
         FROM goose_db_version
         WHERE version_id > 0 AND is_applied = 1;",
    )
    .map_err(|e| NeboError::Migration(format!("failed to reconcile goose versions: {e}")))?;

    Ok(())
}

/// Extract the version number from a migration filename.
/// "0001_initial_schema.sql" -> Some(1)
fn extract_version(filename: &str) -> Option<i64> {
    filename.split('_').next()?.parse::<i64>().ok()
}

/// Extract only the "Up" portion of a goose migration file.
/// Splits on `-- +goose Down` and returns only the content after `-- +goose Up`.
fn extract_goose_up(sql: &str) -> String {
    let mut in_up = false;
    let mut up_lines = Vec::new();

    for line in sql.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("-- +goose Up") {
            in_up = true;
            continue;
        }
        if trimmed.starts_with("-- +goose Down") {
            break;
        }
        if in_up {
            up_lines.push(line);
        }
    }

    // If no goose markers found, use the whole file
    if up_lines.is_empty() && !sql.contains("-- +goose") {
        return sql.to_string();
    }

    up_lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_version() {
        assert_eq!(extract_version("0001_initial_schema.sql"), Some(1));
        assert_eq!(extract_version("0046_session_work_tasks.sql"), Some(46));
        assert_eq!(extract_version("invalid.sql"), None);
    }

    #[test]
    fn test_extract_goose_up() {
        let sql = "-- +goose Up\nCREATE TABLE foo (id INT);\n-- +goose Down\nDROP TABLE foo;";
        assert_eq!(extract_goose_up(sql), "CREATE TABLE foo (id INT);");
    }

    #[test]
    fn test_migrations_embedded() {
        let files = iter_files();
        assert!(!files.is_empty(), "should have embedded migration files");
        assert!(
            files.iter().any(|f| f.starts_with("0001")),
            "should have initial migration"
        );
    }

    #[test]
    fn test_run_migrations_in_memory() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Verify migrations table exists and has entries
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM _nebo_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(count > 0, "should have applied migrations");
    }
}

#[cfg(test)]
mod idempotency_tests {
    use super::*;

    /// Running the full migration chain twice on the same on-disk DB is a
    /// no-op the second time: no error, no re-applied migrations, no
    /// duplicate schema objects. This is the restart path of every install.
    /// The upgrade an install like Danny's makes: a database in the shape
    /// before the engine (version 142), holding a cron job with its history
    /// and last run, a seen inbound message, a sub-agent fan-out, a
    /// workflow run parked on an approval, one interrupted, one finished.
    /// The upgrade writes its copy first, carries every one of those into
    /// the engine's rows, drops the tables and columns it replaced, and is
    /// idempotent when the store opens it again.
    #[test]
    fn an_install_at_the_pre_engine_shape_upgrades_with_its_history_carried() {
        let path = std::env::temp_dir().join(format!("nebo-upgrade-{}.db", uuid::Uuid::new_v4()));
        let path_s = path.to_string_lossy().to_string();
        let conn = Connection::open(&path).unwrap();
        run_migrations_to(&conn, 142).unwrap();
        let applied: i64 = conn.query_row("SELECT MAX(version) FROM _nebo_migrations", [], |r| r.get(0)).unwrap();
        assert_eq!(applied, 142);
        assert!(conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name = 'engine_runs'", [], |r| r.get::<_, i64>(0)).unwrap() == 0, "no engine yet");

        conn.execute_batch(
            "INSERT INTO cron_jobs (id, name, schedule, command, task_type, enabled, last_run, run_count, agent_id)
                 VALUES (1, 'briefing', '0 0 9 * * *', 'echo hi', 'shell', 1, '2026-09-01 09:00:00', 12, 'ic');
             INSERT INTO cron_history (id, job_id, started_at, finished_at, success, output)
                 VALUES (7, 1, '2026-09-01 09:00:00', '2026-09-01 09:00:10', 1, 'ran');
             INSERT INTO comm_seen_messages (id, seen_at) VALUES ('sms-abc', 1700000000);
             INSERT INTO pending_tasks (id, task_type, status, session_key, prompt, created_at, completed_at)
                 VALUES ('root', 'subagent', 'completed', 'agent:a:web', 'fan out', 1700000000, 1700000100);
             INSERT INTO pending_tasks (id, task_type, status, session_key, prompt, created_at, parent_task_id)
                 VALUES ('child', 'subagent', 'pending', 'agent:a:web', 'part one', 1700000001, 'root');
             INSERT INTO pending_tasks (id, task_type, status, session_key, prompt, created_at)
                 VALUES ('track', 'tracking', 'pending', 'agent:a:web', 'watch', 1700000002);
             INSERT INTO workflow_runs (id, workflow_id, trigger_type, status, inputs, session_key, definition, started_at)
                 VALUES ('wf-park', 'agent:ic', 'watch', 'awaiting_approval', '{}', 'agent:ic:workflow:wf-park', '{\"activities\":[]}', 1700000000);
             INSERT INTO workflow_run_suspensions (run_id, agent_id, binding_name, activity_id, iteration, step_index, messages, pending_tool, operation, display, created_at)
                 VALUES ('wf-park', 'ic', 'watch', 'act-2', 0, 0, '[]', '{}', 'crm.write', 'Create invoice', 1700000050);
             INSERT INTO workflow_runs (id, workflow_id, trigger_type, status, session_key, definition, started_at)
                 VALUES ('wf-int', 'agent:ic', 'manual', 'interrupted', 'agent:ic:workflow:wf-int', '{}', 1700000000);
             INSERT INTO workflow_runs (id, workflow_id, trigger_type, status, output, started_at, completed_at)
                 VALUES ('wf-done', 'agent:ic', 'manual', 'completed', 'done', 1700000000, 1700000200);",
        )
        .unwrap();

        run_migrations(&conn).unwrap();

        // The copy, in the old shape, written before anything changed.
        let backup = format!("{path_s}.pre-v0142.bak");
        assert!(std::path::Path::new(&backup).exists(), "pre-migration copy");
        let old = Connection::open(&backup).unwrap();
        assert_eq!(old.query_row("SELECT COUNT(*) FROM cron_history", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
        assert_eq!(old.query_row("SELECT MAX(version) FROM _nebo_migrations", [], |r| r.get::<_, i64>(0)).unwrap(), 142);

        let one = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, String>(0)).unwrap();
        let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        // The cron history is a task run bound to the job; the last run is the timer floor.
        assert_eq!(one("SELECT state || ' ' || external_ref FROM engine_runs WHERE id = 'cron-legacy-7'"), "done cron:1");
        assert_eq!(count("SELECT COUNT(*) FROM engine_events WHERE kind = 'timer' AND target_id = 'cron:1' AND delivered_at IS NOT NULL"), 1);
        assert_eq!(count("SELECT COUNT(*) FROM pragma_table_info('cron_jobs') WHERE name IN ('last_run', 'run_count', 'last_error')"), 0, "the old columns are gone");
        // The seen inbound message is a seen event under the same key.
        assert_eq!(count("SELECT COUNT(*) FROM engine_events WHERE kind = 'seen' AND idem_key = 'comm:sms-abc' AND delivered_at IS NOT NULL"), 1);
        // The fan-out keeps its parent link and states; tracking rows stay where they were.
        assert_eq!(one("SELECT state FROM engine_runs WHERE id = 'root'"), "done");
        assert_eq!(one("SELECT state || ' ' || parent_run_id FROM engine_runs WHERE id = 'child'"), "queued root");
        assert_eq!(count("SELECT COUNT(*) FROM pending_tasks"), 1, "only the tracking row remains");
        // The parked workflow is a waiting run with its approval as its live wait.
        assert_eq!(one("SELECT state FROM engine_runs WHERE id = 'wf-park'"), "waiting");
        assert_eq!(one("SELECT w.action || ' ' || w.on_kind || ' ' || w.key FROM engine_waits w JOIN engine_runs r ON r.current_wait_id = w.id WHERE r.id = 'wf-park'"), "resume approval approval:wf-park");
        assert!(one("SELECT parked FROM engine_waits WHERE run_id = 'wf-park'").contains("crm.write"));
        assert_eq!(one("SELECT state FROM engine_runs WHERE id = 'wf-int'"), "interrupted");
        assert_eq!(one("SELECT state || ' ' || result FROM engine_runs WHERE id = 'wf-done'"), "done done");
        for gone in ["cron_history", "comm_seen_messages", "workflow_run_suspensions"] {
            assert_eq!(count(&format!("SELECT COUNT(*) FROM sqlite_master WHERE name = '{gone}'")), 0, "{gone} is gone");
        }
        assert_eq!(count("SELECT COUNT(*) FROM pragma_table_info('workflow_runs') WHERE name IN ('status', 'inputs', 'definition', 'resume_attempted')"), 0);
        assert_eq!(count("SELECT COUNT(*) FROM pragma_table_info('workflow_runs') WHERE name = 'model'"), 1);
        drop(conn);

        // The store opens the upgraded file and finds nothing more to do.
        let store = crate::Store::new(&path_s).unwrap();
        assert_eq!(store.engine_get_run("wf-park").unwrap().unwrap().state, "waiting");
        let queued = store.engine_queued_runs("main", 10).unwrap();
        assert_eq!(queued.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["child"], "the pending child is queued on its old lane, ready to run");
    }

    #[test]
    fn run_migrations_twice_is_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nebo-migrate-test.db");
        let conn = Connection::open(&path).unwrap();

        run_migrations(&conn).unwrap();

        let count_rows = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        let applied_1 = count_rows("SELECT COUNT(*) FROM _nebo_migrations");
        let objects_1 = count_rows("SELECT COUNT(*) FROM sqlite_master");

        // Every embedded migration file must have been applied exactly once.
        assert_eq!(applied_1, iter_files().len() as i64);

        run_migrations(&conn).expect("second run must not error");

        assert_eq!(
            count_rows("SELECT COUNT(*) FROM _nebo_migrations"),
            applied_1,
            "second run must not re-apply migrations"
        );
        assert_eq!(
            count_rows("SELECT COUNT(*) FROM sqlite_master"),
            objects_1,
            "second run must not create duplicate schema objects"
        );
    }

    /// A migration file without goose markers is applied verbatim — the
    /// whole file is the Up script, not silently skipped.
    #[test]
    fn extract_goose_up_without_markers_uses_whole_file() {
        let sql = "CREATE TABLE bare (id INT);";
        assert_eq!(extract_goose_up(sql), sql);
    }
}
