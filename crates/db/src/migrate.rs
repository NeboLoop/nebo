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

/// The highest migration version this binary carries. Derived from the
/// embedded files, so it is whatever actually ships — never a number a
/// caller has to keep in step by hand.
pub fn head_version() -> i64 {
    versions().last().copied().unwrap_or(0)
}

/// Every migration version this binary carries, in order. Parallel branches
/// take numbers as they land, so the list may skip one; the migrator applies
/// each version it has not applied, whatever its neighbours.
pub fn versions() -> Vec<i64> {
    let mut versions: Vec<i64> = iter_files().iter().filter_map(|f| extract_version(f)).collect();
    versions.sort_unstable();
    versions
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
    reconcile_renumbered(conn)?;

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
            // The ONE copy primitive: consistent, and verified before it counts.
            match crate::backup::vacuum_into(conn, std::path::Path::new(&backup)) {
                Ok(_) => info!(backup = %backup, from_version = from, pending = pending.len(), "pre-migration database copy written and verified"),
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

/// A migration is its name; its number is only its order. When a file is
/// renumbered (a parallel branch had taken the number), a database that
/// applied it under the old number keeps it applied under the new one: the
/// record moves with the file, so the migration never runs twice and the old
/// number is free for the migration that owns it now. A record moves only
/// when nothing is recorded at the new number yet.
fn reconcile_renumbered(conn: &Connection) -> Result<(), NeboError> {
    for filename in iter_files() {
        let (Some(version), Some(slug)) = (extract_version(&filename), migration_slug(&filename)) else {
            continue;
        };
        let moved = conn
            .execute(
                "UPDATE _nebo_migrations SET version = ?1, name = ?2
                 WHERE version = (
                     SELECT MIN(version) FROM _nebo_migrations
                     WHERE substr(name, instr(name, '_') + 1) = ?3 AND version != ?1
                 )
                 AND NOT EXISTS (SELECT 1 FROM _nebo_migrations WHERE version = ?1)",
                rusqlite::params![version, filename, slug],
            )
            .map_err(|e| NeboError::Migration(format!("failed to reconcile renumbered {filename}: {e}")))?;
        if moved > 0 {
            info!(version, filename, "migration renumbered; its applied record follows it");
        }
    }
    Ok(())
}

/// The name of a migration without its number.
/// "0184_chat_linked_chat_id.sql" -> Some("chat_linked_chat_id.sql")
fn migration_slug(filename: &str) -> Option<&str> {
    filename.split_once('_').map(|(_, slug)| slug)
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

    /// Two files with one version number: the runner applies both, records
    /// the first, and fails recording the second (`_nebo_migrations.version`
    /// is unique), so every bot exits once at boot and then runs without the
    /// second file's schema (2026-09-24: 0163_binding_standing_outcome vs
    /// 0163_comm_stream_offsets took the whole auto-update fleet through
    /// that). Parallel branches each take "the next number"; this is the
    /// check that catches it before an image does.
    #[test]
    fn no_two_migrations_share_a_version() {
        let mut seen = std::collections::HashMap::new();
        for f in iter_files() {
            let v = extract_version(&f).unwrap_or_else(|| panic!("invalid migration filename: {f}"));
            if let Some(prev) = seen.insert(v, f.clone()) {
                panic!("migration version {v} is used twice: {prev} and {f} — renumber the newer one");
            }
        }
    }

    /// A migration is identified by its name when it is renumbered
    /// (`reconcile_renumbered`), so no two files may share one.
    #[test]
    fn no_two_migrations_share_a_name() {
        let mut seen = std::collections::HashMap::new();
        for f in iter_files() {
            let slug = migration_slug(&f).unwrap_or_else(|| panic!("invalid migration filename: {f}")).to_string();
            if let Some(prev) = seen.insert(slug, f.clone()) {
                panic!("migration name is used twice: {prev} and {f} — name the newer one for what it does");
            }
        }
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
        // The one loop's upgrade (0173) then fails the child the old
        // orchestrator never ran; both become helper rows.
        assert_eq!(one("SELECT state || ' ' || parent_run_id FROM engine_runs WHERE id = 'child'"), "failed root");
        assert_eq!(one("SELECT kind FROM engine_runs WHERE id = 'root'"), "helper");
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
        assert!(store.engine_queued_runs("main", 10).unwrap().is_empty(), "nothing waits on the old orchestrator");
    }

    /// Every visible checkpoint row becomes a hidden boundary and one
    /// owner-visible marker just before it; the model's conversation still
    /// opens on the boundary and never holds a marker. Running it again
    /// changes nothing.
    #[test]
    fn visible_checkpoint_rows_become_hidden_with_one_marker_each() {
        let path = std::env::temp_dir().join(format!("nebo-upgrade-{}.db", uuid::Uuid::new_v4()));
        let conn = Connection::open(&path).unwrap();
        run_migrations_to(&conn, 182).unwrap();
        conn.execute_batch(
            "INSERT INTO chats (id, title) VALUES ('c', 'C');
             INSERT INTO chat_messages (id, chat_id, role, content, created_at) VALUES ('u1', 'c', 'user', 'Build the billing employee.', 100);
             INSERT INTO chat_messages (id, chat_id, role, content, metadata, created_at) VALUES ('b1', 'c', 'user', 'This conversation continues from an earlier part that was summarized:\n\nfirst', '{\"checkpoint\":true,\"reason\":\"threshold\"}', 200);
             INSERT INTO chat_messages (id, chat_id, role, content, created_at) VALUES ('a1', 'c', 'assistant', 'Working.', 201);
             INSERT INTO chat_messages (id, chat_id, role, content, metadata, created_at) VALUES ('b2', 'c', 'user', 'This conversation continues from an earlier part that was summarized:\n\nsecond', '{\"checkpoint\":true,\"reason\":\"overflow\"}', 300);
             INSERT INTO chat_messages (id, chat_id, role, content, created_at) VALUES ('u2', 'c', 'user', 'Keep going.', 301);",
        )
        .unwrap();

        run_migrations_to(&conn, 183).unwrap();
        let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        let visible = "COALESCE(json_extract(metadata, '$.isMeta'), 0) NOT IN (1, 'true')";
        assert_eq!(count(&format!("SELECT COUNT(*) FROM chat_messages WHERE content LIKE 'This conversation continues%' AND {visible}")), 0, "no summary is visible");
        assert_eq!(count("SELECT COUNT(*) FROM chat_messages WHERE role = 'system' AND json_extract(metadata, '$.compactBoundary') = 1"), 2, "one marker per checkpoint");
        assert_eq!(count("SELECT COUNT(*) FROM chat_messages WHERE json_extract(metadata, '$.reason') = 'overflow' AND role = 'system'"), 1, "the marker keeps the reason");
        // Idempotent: the migration's statements, run again, change nothing.
        conn.execute_batch(include_str!("../migrations/0183_checkpoint_summaries_hidden.sql")).unwrap();
        assert_eq!(count("SELECT COUNT(*) FROM chat_messages"), 7);
        drop(conn);

        let store = crate::Store::new(&path.to_string_lossy()).unwrap();
        let loaded = store.get_chat_messages_since_checkpoint("c").unwrap();
        assert_eq!(loaded.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["b2", "u2"], "the model opens on the boundary, no marker");
        let thread: Vec<String> = store.get_chat_messages("c").unwrap().iter().map(|m| format!("{}:{}", m.role, m.content.lines().next().unwrap_or(""))).collect();
        assert_eq!(thread[0], "user:Build the billing employee.");
        assert_eq!(thread[1], "system:Earlier conversation summarized", "the marker sits before its boundary: {thread:?}");
    }

    /// Each rolling summary becomes one checkpoint boundary row in its
    /// session's active chat, placed before the last 80 visible rows (the
    /// most the sliding window held) or before every row when there are
    /// fewer, and the summary is cleared. The text is the boundary the
    /// harness writes after `/compact` (`checkpoint::boundary_text`).
    #[test]
    fn a_rolling_summary_becomes_a_checkpoint_boundary() {
        let path = std::env::temp_dir().join(format!("nebo-upgrade-{}.db", uuid::Uuid::new_v4()));
        let conn = Connection::open(&path).unwrap();
        run_migrations_to(&conn, 167).unwrap();
        conn.execute_batch(
            "INSERT INTO chats (id, title) VALUES ('long', 'Long'), ('short', 'Short');
             INSERT INTO sessions (id, name, active_chat_id, summary, created_at, updated_at) VALUES ('s-long', 'agent:a:web', 'long', 'Owner wants the Q3 report.', 1, 1);
             INSERT INTO sessions (id, name, active_chat_id, summary, created_at, updated_at) VALUES ('s-short', 'agent:b:web', 'short', 'Owner asked for a haiku.', 1, 1);
             INSERT INTO sessions (id, name, active_chat_id, summary, created_at, updated_at) VALUES ('s-none', 'agent:c:web', NULL, '  ', 1, 1);",
        )
        .unwrap();
        for i in 0..100 {
            conn.execute(
                "INSERT INTO chat_messages (id, chat_id, role, content, created_at) VALUES (?1, 'long', 'user', 'm', ?2)",
                rusqlite::params![format!("l{i:03}"), 1_700_000_000 + i],
            )
            .unwrap();
        }
        for i in 0..3 {
            conn.execute(
                "INSERT INTO chat_messages (id, chat_id, role, content, created_at) VALUES (?1, 'short', 'user', 'm', ?2)",
                rusqlite::params![format!("s{i}"), 1_700_000_000 + i],
            )
            .unwrap();
        }

        run_migrations_to(&conn, 168).unwrap();

        let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(count("SELECT COUNT(*) FROM sessions WHERE summary IS NOT NULL AND trim(summary) != ''"), 0, "every summary moved");
        assert_eq!(count("SELECT COUNT(*) FROM chat_messages WHERE json_extract(metadata, '$.checkpoint') = 1"), 2, "one boundary per summary");
        drop(conn);

        let store = crate::Store::new(&path.to_string_lossy()).unwrap();
        let long = store.get_chat_messages_since_checkpoint("long").unwrap();
        assert_eq!(long.len(), 81, "the boundary and the 80 rows the window held");
        assert_eq!(long[0].role, "user");
        assert_eq!(
            long[0].content,
            "This conversation continues from an earlier part that was summarized:\n\nOwner wants the Q3 report.\n\n\
             If you need a specific detail from before this summary (an exact snippet, an error message, something you \
             wrote), the earlier conversation is still stored: search it with \
             search_history(query: \"...\")."
        );
        assert_eq!(long[1].id, "l020");
        let short = store.get_chat_messages_since_checkpoint("short").unwrap();
        assert_eq!(short.len(), 4, "every row stays after the boundary");
        assert!(short[0].content.contains("Owner asked for a haiku."));
        assert_eq!(store.get_chat_messages("long").unwrap().len(), 102, "the thread keeps every row, and 0183 marks the boundary");
    }

    /// Owner rule (09-25): an employee reachable from outside is a
    /// multi-chat employee. The upgrade converts the ones already bound — a
    /// channel binding (Slack, the phone line's bridge) or loop exposure —
    /// leaves the unbound alone, and is idempotent.
    #[test]
    fn the_upgrade_makes_every_bound_employee_multi_chat() {
        let path = std::env::temp_dir().join(format!("nebo-upgrade-{}.db", uuid::Uuid::new_v4()));
        let conn = Connection::open(&path).unwrap();
        run_migrations_to(&conn, 178).unwrap();
        conn.execute_batch(
            "INSERT INTO agents (id, name, description, agent_md, frontmatter, loop_exposed) VALUES
               ('desk', 'Front Desk', '', '', '{}', 0),
               ('scout', 'Scout', '', '', '{}', 1),
               ('quiet', 'Quiet', '', '', '{}', 0),
               ('paused', 'Paused', '', '', '{}', 0);
             INSERT INTO channel_bindings (agent_id, plugin_slug, is_enabled) VALUES
               ('desk', 'phonecall', 1),
               ('paused', 'slack', 0);
             INSERT INTO entity_config (entity_type, entity_id, multi_chat, model_preference) VALUES
               ('agent', 'desk', 0, 'janus/fast');",
        )
        .unwrap();
        run_migrations_to(&conn, 179).unwrap();
        let multi = |id: &str| -> Option<i64> {
            conn.query_row(
                "SELECT multi_chat FROM entity_config WHERE entity_type = 'agent' AND entity_id = ?1",
                [id],
                |r| r.get(0),
            )
            .ok()
        };
        assert_eq!(multi("desk"), Some(1), "a bound single-chat employee is converted");
        assert_eq!(multi("scout"), Some(1), "loop exposure is a door; a row is created");
        assert_eq!(multi("quiet"), None, "an unbound employee is untouched");
        assert_eq!(multi("paused"), None, "a switched-off channel is not a door");
        let model: String = conn
            .query_row("SELECT model_preference FROM entity_config WHERE entity_id = 'desk'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(model, "janus/fast", "the rest of the row is kept");
        let snapshot = || -> Vec<(String, Option<i64>)> {
            let mut stmt = conn.prepare("SELECT entity_id, multi_chat FROM entity_config ORDER BY entity_id").unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
        };
        let before = snapshot();
        conn.execute_batch(include_str!("../migrations/0179_bound_employees_are_multi_chat.sql")).unwrap();
        assert_eq!(snapshot(), before, "running it again changes nothing");
    }

    /// The tools each MCP server offered at its last sync carry over from
    /// the old per-server permissions, so nothing the owner already saw is
    /// new at the upgrade; a server with no list starts with none.
    #[test]
    fn an_mcp_servers_known_tools_carry_over() {
        let path = std::env::temp_dir().join(format!("nebo-upgrade-{}.db", uuid::Uuid::new_v4()));
        let conn = Connection::open(&path).unwrap();
        run_migrations_to(&conn, 176).unwrap();
        conn.execute_batch(
            r#"INSERT INTO mcp_integrations (id, name, server_type, auth_type, tool_permissions)
                 VALUES ('seen', 'CRM', 'crm', 'none', '{"default":"allow","tools":{},"known":["lookup","update"]}');
               INSERT INTO mcp_integrations (id, name, server_type, auth_type, tool_permissions)
                 VALUES ('never', 'Docs', 'docs', 'none', NULL);"#,
        )
        .unwrap();
        run_migrations_to(&conn, 177).unwrap();
        drop(conn);
        let store = crate::Store::new(&path.to_string_lossy()).unwrap();
        assert_eq!(
            store.get_mcp_known_tools("seen").unwrap(),
            vec!["lookup", "update"]
        );
        assert!(store.get_mcp_known_tools("never").unwrap().is_empty());
    }

    /// The one loop's upgrade: the steering the old loop stored in threads
    /// is deleted and everything else stays; the orchestrator's rows become
    /// helper rows, a live one failed; the old session state columns go.
    #[test]
    fn the_one_loop_upgrade_deletes_stored_steering_and_retires_the_old_rows() {
        let path = std::env::temp_dir().join(format!("nebo-upgrade-{}.db", uuid::Uuid::new_v4()));
        let conn = Connection::open(&path).unwrap();
        run_migrations_to(&conn, 172).unwrap();
        conn.execute_batch(
            r#"INSERT INTO chats (id, title) VALUES ('c', 'C');
             INSERT INTO chat_messages (id, chat_id, role, content, metadata, created_at) VALUES
               ('owner', 'c', 'user', 'Send the invoices.', NULL, 1),
               ('nudge', 'c', 'user', '  Continue — your previous response committed to more work that isn''t done yet: the invoices. Keep going and finish it.', NULL, 2),
               ('stamped', 'c', 'user', 'keep going', '{"autoContinue":true}', 3),
               ('budget', 'c', 'user', 'You''ve reached the maximum number of tool-calling iterations allowed. Please provide a final response summarizing what you''ve found and accomplished so far, without calling any more tools.', NULL, 4),
               ('room', 'c', 'user', 'Team Ops', '{"roomBriefing":true}', 5),
               ('queued', 'c', 'user', '<system-reminder>
Team Ops
</system-reminder>', '{"isMeta":true}', 6),
               ('attachment', 'c', 'user', '<system-reminder>
Date
</system-reminder>', '{"attachment":{"kind":"date_changed"},"isMeta":true}', 7),
               ('notification', 'c', 'user', '<system-reminder>
[Notification: not a message from the owner]
</system-reminder>', '{"notification":true,"isMeta":true}', 8),
               ('boundary', 'c', 'user', 'This conversation continues', '{"checkpoint":true}', 9),
               ('odd', 'c', 'user', '<system-reminder>not json</system-reminder>', 'not json', 10),
               ('reply', 'c', 'assistant', '<system-reminder> quoted', '{"isMeta":true}', 11);
             INSERT INTO sessions (id, name, active_chat_id, active_task, created_at, updated_at) VALUES ('s', 'agent:a:web', 'c', 'invoices', 1, 1);
             INSERT INTO engine_runs (id, kind, state, session_key, lane) VALUES
               ('live', 'subagent', 'running', 'subagent:agent:a:web:live', 'subagent'),
               ('done', 'subagent', 'done', 'subagent:agent:a:web:done', 'subagent'),
               ('job', 'dag', 'queued', 'agent:a:web', 'subagent'),
               ('wf', 'workflow', 'running', 'agent:a:workflow:wf', 'main');"#,
        )
        .unwrap();

        run_migrations(&conn).unwrap();

        let ids: Vec<String> = conn
            .prepare("SELECT id FROM chat_messages ORDER BY created_at")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(ids, ["owner", "attachment", "notification", "boundary", "odd", "reply"]);
        let one = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, String>(0)).unwrap();
        assert_eq!(one("SELECT kind || ' ' || state FROM engine_runs WHERE id = 'live'"), "helper failed");
        assert_eq!(one("SELECT kind || ' ' || state FROM engine_runs WHERE id = 'done'"), "helper done");
        assert_eq!(one("SELECT kind || ' ' || state FROM engine_runs WHERE id = 'job'"), "helper failed");
        assert_eq!(one("SELECT kind || ' ' || state FROM engine_runs WHERE id = 'wf'"), "workflow running", "other runs are untouched");
        let gone: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name IN ('active_task', 'summary', 'last_summarized_count', 'work_tasks')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(gone, 0, "the old loop's session columns are gone");
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

    /// A database that applied `chat_linked_chat_id` as 0166 (before it was
    /// renumbered to 0184) opens with its record moved to 0184: the column is
    /// not added twice, 0166 is free for the migration that owns that number,
    /// and every migration this binary carries is recorded once.
    #[test]
    fn a_migration_applied_under_its_old_number_is_not_applied_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = Connection::open(dir.path().join("renumbered.db")).unwrap();
        run_migrations_to(&conn, 165).unwrap();
        // What the old 0166 did, recorded under its old number.
        conn.execute_batch(
            "ALTER TABLE chats ADD COLUMN linked_chat_id TEXT;
             INSERT INTO _nebo_migrations (version, name) VALUES (166, '0166_chat_linked_chat_id.sql');",
        )
        .unwrap();

        run_migrations(&conn).expect("the renumbered migration must not run again");

        let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(count("SELECT COUNT(*) FROM pragma_table_info('chats') WHERE name = 'linked_chat_id'"), 1);
        let owner: String = conn
            .query_row("SELECT name FROM _nebo_migrations WHERE version = 166", [], |r| r.get(0))
            .unwrap();
        assert_eq!(owner, "0166_permissions.sql", "0166 went to the migration that owns it, and ran");
        assert_eq!(count("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'permission_rules'"), 1);
        let name: String = conn
            .query_row("SELECT name FROM _nebo_migrations WHERE version = 184", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "0184_chat_linked_chat_id.sql");
        for v in versions() {
            assert_eq!(count(&format!("SELECT COUNT(*) FROM _nebo_migrations WHERE version = {v}")), 1, "{v} applied");
        }
    }

    /// A database at 0183 (the branch's last before main's linked-chat
    /// migration landed) gets 0184 on top, and every other record stays.
    #[test]
    fn a_database_at_0183_gets_0184() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = Connection::open(dir.path().join("at-0183.db")).unwrap();
        run_migrations_to(&conn, 183).unwrap();
        let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(count("SELECT COUNT(*) FROM pragma_table_info('chats') WHERE name = 'linked_chat_id'"), 0);

        run_migrations(&conn).unwrap();

        assert_eq!(count("SELECT COUNT(*) FROM pragma_table_info('chats') WHERE name = 'linked_chat_id'"), 1);
        assert_eq!(count("SELECT COUNT(*) FROM _nebo_migrations WHERE version = 184"), 1);
        assert_eq!(count("SELECT COUNT(*) FROM _nebo_migrations WHERE version = 166 AND name = '0166_permissions.sql'"), 1, "another migration's record is untouched");
    }

    /// A linked chat stored under the phone contract's id reaches the same
    /// session under Open Agent Link: `<member>~<session>` loses its member,
    /// a first member's raw session stays, and so does anything whose part
    /// before a `~` could not be a member id.
    #[test]
    fn linked_chats_keep_their_sessions_under_open_agent_link() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = Connection::open(dir.path().join("linked.db")).unwrap();
        run_migrations_to(&conn, 184).unwrap();
        for (id, linked) in [
            ("local", Some("claude-code~s-1")),
            ("member", Some("openclaw-2~agent:main:thread-9")),
            ("first", Some("20260925_143012_ab12cd")),
            ("odd", Some("agent:main:x~y")),
            ("none", None),
        ] {
            conn.execute(
                "INSERT INTO chats (id, title, created_at, updated_at, linked_chat_id) VALUES (?1, 't', 0, 0, ?2)",
                rusqlite::params![id, linked],
            )
            .unwrap();
        }

        run_migrations(&conn).unwrap();

        let linked = |id: &str| -> (Option<String>, Option<String>) {
            conn.query_row("SELECT linked_chat_id, linked_agent_id FROM chats WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
        };
        assert_eq!(linked("local"), (Some("s-1".into()), None));
        assert_eq!(linked("member"), (Some("agent:main:thread-9".into()), None));
        assert_eq!(linked("first"), (Some("20260925_143012_ab12cd".into()), None));
        assert_eq!(linked("odd"), (Some("agent:main:x~y".into()), None), "not a member's prefix");
        assert_eq!(linked("none"), (None, None));
    }

    /// Sends Nebo refused before the plugin ran, held as "outcome unknown"
    /// before the send path failed them itself (live 2026-09-26), are
    /// failed on their recorded refusal and their false notices go. A row
    /// whose plugin ran — untyped output, a failure after launch — is not
    /// guessed at, and neither is a row already settled or a charge.
    #[test]
    fn sends_refused_before_the_plugin_ran_are_failed_and_nothing_else_is() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = Connection::open(dir.path().join("presend.db")).unwrap();
        run_migrations_to(&conn, 191).unwrap();
        conn.execute("INSERT INTO users (id, email, password_hash) VALUES ('u1', 'o@example.com', 'x')", []).unwrap();
        let notice = |why: &str| {
            format!(
                "A mail.message.send through gmail was attempted and the plugin reported no typed outcome: {why}. \
                 It was not retried, because it may already have been delivered. Check the provider's sent items; ledger entry #1."
            )
        };
        let refused = "No gmail account is connected for this agent. Connect one in this agent's Settings, Plugins before using gmail. Connected for this employee: shopify. Do the work with what is connected; if it cannot b";
        for (id, class, state, why) in [
            (5, "messaging", "pending", refused.to_string()),
            (6, "messaging", "pending", "Plugin 'gmail' not found. Available: shopify".to_string()),
            (7, "messaging", "pending", "Error: connection refused".to_string()),
            (8, "messaging", "pending", "Plugin 'gmail' command failed: broken pipe".to_string()),
            (9, "messaging", "completed", refused.to_string()),
            (10, "financial", "pending", refused.to_string()),
        ] {
            conn.execute(
                "INSERT INTO engine_effects (id, run_id, class, idem_key, provider, state, attempts) VALUES (?1, 'run-1', ?2, ?3, 'gmail', ?4, 1)",
                rusqlite::params![id, class, format!("send:run-1:mail.message.send:{id}"), state],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO notifications (id, user_id, type, title, body) VALUES (?1, 'u1', 'needs_attention', 'A send could not be confirmed', ?2)",
                rusqlite::params![format!("attention:effect:{id}"), notice(&why)],
            )
            .unwrap();
        }

        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap();

        let row = |id: i64| -> (String, Option<String>) {
            conn.query_row("SELECT state, result FROM engine_effects WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
        };
        let notice_kept = |id: i64| -> bool {
            conn.query_row("SELECT COUNT(*) FROM notifications WHERE id = ?1", [format!("attention:effect:{id}")], |r| r.get::<_, i64>(0)).unwrap() == 1
        };
        for id in [5, 6] {
            let (state, result) = row(id);
            assert_eq!(state, "failed", "row {id}");
            let result = result.unwrap_or_default();
            assert!(result.starts_with("Not sent: Nebo refused it before the plugin ran. "), "{result}");
            assert!(!result.contains("It was not retried"), "the refusal alone: {result}");
            assert!(!notice_kept(id), "the false notice for {id} goes");
        }
        assert!(row(5).1.unwrap().contains("No gmail account is connected"));
        for (id, state) in [(7, "pending"), (8, "pending"), (9, "completed"), (10, "pending")] {
            assert_eq!(row(id).0, state, "row {id} is not guessed at");
            assert!(notice_kept(id), "row {id}'s notice stays");
        }
    }

    /// The team-post copies stored in members' threads leave them (the
    /// owner's case, 2026-09-26: Neighbor Mail's chat held the owner's post
    /// to Marketing & Growth and the Social Media Manager's answer). A copy
    /// whose post is in the team thread goes; a copy whose original is
    /// missing moves into the team thread as the team row it copied; the
    /// restart note a copy caused goes. The team thread keeps its whole
    /// history, an asked member's own work in its seat stays, and so does
    /// everything in the member's direct chat that is its own. Running it
    /// again changes nothing.
    #[test]
    fn team_post_copies_leave_member_threads_and_the_team_history_keeps_them() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = Connection::open(dir.path().join("team-copies.db")).unwrap();
        run_migrations_to(&conn, 186).unwrap();
        let envelope = |from: &str, text: &str| {
            format!("[Team \"Marketing & Growth\" — Grow the pipeline]\n[Post from {from}]\n\n{text}")
        };
        let copy = r#"{"teamId":"t1","teamPost":true}"#;
        let team_row = |from: &str, id: &str| format!(r#"{{"senderName":"{from}","fromAgentId":"{id}","teamId":"t1","attachments":[]}}"#);
        conn.execute_batch(
            "INSERT INTO agents (id, kind, name, description, agent_md, frontmatter) VALUES
                ('smm', 'user', 'Social Media Manager', '', '', '{}'),
                ('hermes', 'user', 'Hermes', '', '', '{}'),
                ('nm', 'user', 'Neighbor Mail', '', '', '{}');
             INSERT INTO sessions (id, name, active_chat_id, created_at, updated_at) VALUES
                ('s-team', 'team:t1', 'team-chat', 0, 0),
                ('s-seat-nm', 'agent:nm:coworker:team:t1', 'seat-nm', 0, 0),
                ('s-seat-smm', 'agent:smm:coworker:team:t1', 'seat-smm', 0, 0),
                ('s-direct', 'agent:nm:web', 'agent:nm:web', 0, 0);
             INSERT INTO chats (id, title, session_name) VALUES
                ('team-chat', 'Marketing & Growth', 'team:t1'),
                ('seat-nm', 'Team: Marketing & Growth', 'agent:nm:coworker:team:t1'),
                ('seat-smm', 'Team: Marketing & Growth', 'agent:smm:coworker:team:t1'),
                ('agent:nm:web', 'USPS rates', 'agent:nm:web');",
        )
        .unwrap();
        let insert = |id: &str, chat: &str, role: &str, content: &str, meta: Option<&str>, at: i64| {
            conn.execute(
                "INSERT INTO chat_messages (id, chat_id, role, content, metadata, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![id, chat, role, content, meta, at],
            )
            .unwrap();
        };
        // The team thread: the owner's post and the lead's answer.
        insert("t-owner", "team-chat", "user", "add Hermes to this team", Some(&team_row("Owner", "")), 100);
        insert("t-smm", "team-chat", "assistant", "I need to create Hermes first.", Some(&team_row("Social Media Manager", "smm")), 111);
        // Neighbor Mail was never asked: copies of both, a copy of a Hermes
        // post the team thread no longer has, and the restart note they caused.
        insert("c-owner", "seat-nm", "user", &envelope("Owner", "add Hermes to this team"), Some(copy), 100);
        insert("c-smm", "seat-nm", "user", &envelope("Social Media Manager", "I need to create Hermes first."), Some(copy), 112);
        insert("c-hermes", "seat-nm", "user", &envelope("Hermes", "I'll research the landscape."), Some(copy), 200);
        insert("c-note", "seat-nm", "assistant", "I was interrupted before I could finish.", Some(r#"{"restartNotice":true}"#), 5000);
        // The lead was asked: its own work in its seat stays.
        insert("l-ask", "seat-smm", "user", &envelope("Owner", "add Hermes to this team"), None, 101);
        insert("l-reply", "seat-smm", "assistant", "I need to create Hermes first.", None, 110);
        // Neighbor Mail's direct chat with the owner, plus one stray copy.
        insert("d-ask", "agent:nm:web", "user", "what are the USPS rates?", None, 50);
        insert("d-copy", "agent:nm:web", "user", &envelope("Owner", "add Hermes to this team"), Some(copy), 100);
        insert("d-q", "agent:nm:web", "user", "and for flats?", None, 300);
        insert("d-note", "agent:nm:web", "assistant", "I was interrupted before I could finish.", Some(r#"{"restartNotice":true}"#), 5000);

        run_migrations(&conn).unwrap();

        let ids = |chat: &str| -> Vec<String> {
            let mut stmt = conn.prepare("SELECT id FROM chat_messages WHERE chat_id = ?1 ORDER BY created_at, rowid").unwrap();
            stmt.query_map([chat], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
        };
        assert!(ids("seat-nm").is_empty(), "nothing of the team's is left in Neighbor Mail's seat: {:?}", ids("seat-nm"));
        assert_eq!(ids("agent:nm:web"), ["d-ask", "d-q", "d-note"], "the direct chat keeps its own rows, and a real restart note");
        assert_eq!(ids("seat-smm"), ["l-ask", "l-reply"], "an asked member's own work stays");
        assert_eq!(ids("team-chat"), ["t-owner", "t-smm", "c-hermes"], "the team history keeps every post");
        let (role, content, meta): (String, String, String) = conn
            .query_row("SELECT role, content, metadata FROM chat_messages WHERE id = 'c-hermes'", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(role, "assistant");
        assert_eq!(content, "I'll research the landscape.", "unwrapped into the team row it copied");
        let meta: serde_json::Value = serde_json::from_str(&meta).unwrap();
        assert_eq!(meta["senderName"], "Hermes");
        assert_eq!(meta["fromAgentId"], "hermes");
        assert_eq!(meta["teamId"], "t1");
        assert!(meta.get("teamPost").is_none(), "{meta}");

        // Idempotent: the statements, run again, change nothing.
        let count = || conn.query_row("SELECT COUNT(*) FROM chat_messages", [], |r| r.get::<_, i64>(0)).unwrap();
        let before = count();
        conn.execute_batch(&extract_goose_up(include_str!("../migrations/0187_team_posts_leave_member_threads.sql"))).unwrap();
        assert_eq!(count(), before);
    }

    /// The owner's own conversations stop owning memory: rows a sealed
    /// employee filed under one of the owner's threads or one workflow run
    /// move up to the employee's private memory, the newest row keeps each
    /// key, a duplicate goes, a different older value is kept beside it, and
    /// a conversation with someone else stays sealed.
    #[test]
    fn memories_leave_the_owners_conversations_for_the_employees_private_memory() {
        use rusqlite::OptionalExtension;
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = Connection::open(dir.path().join("memory-scope.db")).unwrap();
        run_migrations_to(&conn, 187).unwrap();
        conn.execute_batch(
            "INSERT INTO sessions (id, name, active_chat_id, created_at, updated_at) VALUES
                ('s1', 'agent:emp:thread:chat-1', 'chat-1', 0, 0),
                ('s2', 'agent:emp:thread:chat-2', 'chat-2', 0, 0),
                ('s3', 'agent:rec:thread:call-1', 'call-1', 0, 0);
             INSERT INTO workflow_runs (id, workflow_id, trigger_type) VALUES ('run-1', 'agent:rec', 'schedule');
             INSERT INTO memories (id, namespace, key, value, metadata, updated_at, user_id) VALUES
                (1, 'project', 'deck', 'The deck is due Friday', NULL, '2026-01-01', 'o:agent:emp:ctx:chat-1'),
                (2, 'project', 'deck', 'The deck moved to Monday', '{\"provenance\":[\"web\"]}', '2026-02-01', 'o:agent:emp:ctx:chat-2'),
                (3, 'tacit/preferences', 'tone', 'Short and plain', NULL, '2026-01-01', 'o:agent:emp:ctx:chat-1'),
                (4, 'tacit/preferences', 'tone', 'Short and plain', NULL, '2026-03-01', 'o:agent:emp'),
                (5, 'project', 'workflow/pull', 'Voicemail sweep runs hourly', NULL, '2026-01-01', 'o:agent:rec:ctx:run-1:pull::0'),
                (6, 'entity/default', 'person/caller', 'Caller asked about a refund', '{\"provenance\":[\"phone\"]}', '2026-01-01', 'o:agent:rec:ctx:call-1'),
                (7, 'project', 'case/deadline', 'Matter 42 files on the 3rd', NULL, '2026-01-01', 'o:agent:emp:ctx:case-42'),
                (8, 'project', 'recipes/soup', 'Lentil soup, 40 minutes', NULL, '2026-01-01', 'o');
             INSERT INTO memory_chunks (id, memory_id, chunk_index, text, user_id) VALUES
                (11, 1, 0, 'The deck is due Friday', 'o:agent:emp:ctx:chat-1'),
                (13, 3, 0, 'Short and plain', 'o:agent:emp:ctx:chat-1');",
        )
        .unwrap();

        run_migrations(&conn).unwrap();

        let row = |id: i64| -> Option<(String, String)> {
            conn.query_row("SELECT key, user_id FROM memories WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()
                .unwrap()
        };
        let own = |id: i64, key: &str, scope: &str| assert_eq!(row(id), Some((key.to_string(), scope.to_string())), "row {id}");
        own(2, "deck", "o:agent:emp");
        own(1, "deck/earlier-1", "o:agent:emp");
        own(4, "tone", "o:agent:emp");
        assert_eq!(row(3), None, "a duplicate of the kept value goes");
        own(5, "workflow/pull", "o:agent:rec");
        own(6, "person/caller", "o:agent:rec:ctx:call-1");
        own(7, "case/deadline", "o:agent:emp:ctx:case-42");
        own(8, "recipes/soup", "o");
        let chunk = |id: i64| -> Option<String> {
            conn.query_row("SELECT user_id FROM memory_chunks WHERE id = ?1", [id], |r| r.get(0)).optional().unwrap()
        };
        assert_eq!(chunk(11).as_deref(), Some("o:agent:emp"), "a chunk follows its memory");
        assert_eq!(chunk(13), None, "a removed duplicate's chunk goes with it");

        // Idempotent: the statements, run again, change nothing.
        let count = || conn.query_row("SELECT COUNT(*) FROM memories WHERE user_id LIKE '%:ctx:%'", [], |r| r.get::<_, i64>(0)).unwrap();
        let before = (count(), conn.query_row("SELECT COUNT(*) FROM memories", [], |r| r.get::<_, i64>(0)).unwrap());
        conn.execute_batch(&extract_goose_up(include_str!("../migrations/0188_memory_leaves_the_owners_conversations.sql"))).unwrap();
        let after = (count(), conn.query_row("SELECT COUNT(*) FROM memories", [], |r| r.get::<_, i64>(0)).unwrap());
        assert_eq!(before, after);
        assert_eq!(before.0, 2, "only the conversations with someone else stay sealed");
    }

    /// The automation notices stored before 0196 carry the automation marker,
    /// and nothing else does: the employee's own words that merely mention
    /// an automation, and a row with metadata of its own, keep theirs.
    #[test]
    fn stored_automation_notices_are_marked() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = Connection::open(dir.path().join("automation.db")).unwrap();
        run_migrations_to(&conn, 195).unwrap();
        conn.execute_batch(
            r#"INSERT INTO chats (id, title, created_at, updated_at) VALUES ('c', 'c', 0, 0);
               INSERT INTO chat_messages (id, chat_id, role, content, metadata, created_at) VALUES
                ('s', 'c', 'assistant', '**Automation started** — intake (schedule)', NULL, 1),
                ('f', 'c', 'assistant', '**Automation failed** — intake (schedule): no balance', '{"x":1}', 2),
                ('d', 'c', 'assistant', '**Automation completed** — intake (schedule)

Two leads.', NULL, 3),
                ('p', 'c', 'assistant', '**Automation paused for your approval** — send (manual): email', NULL, 4),
                ('w', 'c', 'assistant', 'The **Automation started** line is the schedule firing.', NULL, 5),
                ('u', 'c', 'user', '**Automation started** — intake (schedule)', NULL, 6);"#,
        )
        .unwrap();
        conn.execute_batch(&extract_goose_up(include_str!("../migrations/0196_automation_notices_not_context.sql"))).unwrap();
        let marked = |id: &str| -> bool {
            conn.query_row("SELECT COALESCE(json_extract(metadata, '$.automation'), 0) FROM chat_messages WHERE id = ?1", [id], |r| r.get::<_, i64>(0)).unwrap() == 1
        };
        for id in ["s", "f", "d", "p"] {
            assert!(marked(id), "{id} is a notice");
        }
        for id in ["w", "u"] {
            assert!(!marked(id), "{id} is not");
        }
        let kept: i64 = conn.query_row("SELECT json_extract(metadata, '$.x') FROM chat_messages WHERE id = 'f'", [], |r| r.get(0)).unwrap();
        assert_eq!(kept, 1, "a row's own metadata stays");
    }

    /// Every conversation stored before 0200 starts read at its newest
    /// reply (an automation's notice is not one), and a chat with no reply
    /// has nothing read.
    #[test]
    fn existing_conversations_start_read_at_their_newest_reply() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = Connection::open(dir.path().join("chat-read.db")).unwrap();
        run_migrations_to(&conn, 199).unwrap();
        conn.execute_batch(
            r#"INSERT INTO chats (id, title, session_name, created_at, updated_at) VALUES
                ('c', 'c', 'agent:ap:web', 0, 0), ('q', 'q', 'agent:ap:thread:q', 0, 0);
               INSERT INTO chat_messages (id, chat_id, role, content, metadata, created_at) VALUES
                ('a1', 'c', 'assistant', 'first', NULL, 1),
                ('a2', 'c', 'assistant', 'second', NULL, 2),
                ('s', 'c', 'assistant', '**Automation started** — x', '{"automation":true}', 3),
                ('u', 'c', 'user', 'thanks', NULL, 4),
                ('qu', 'q', 'user', 'hello?', NULL, 1);"#,
        )
        .unwrap();
        run_migrations(&conn).unwrap();
        let read = |id: &str| -> Option<String> {
            conn.query_row("SELECT read_message_id FROM chats WHERE id = ?1", [id], |r| r.get(0)).unwrap()
        };
        assert_eq!(read("c").as_deref(), Some("a2"));
        assert_eq!(read("q"), None);
    }

    /// The isolation flag becomes the memory mode it behaved as: off is one
    /// conversation, on is separate conversations (so the primary employee,
    /// sealed today, stays exactly as it is). A mode already named is kept,
    /// the flag goes everywhere, and nothing else in the frontmatter moves.
    #[test]
    fn the_isolation_flag_becomes_the_memory_mode_it_behaved_as() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = Connection::open(dir.path().join("memory-mode.db")).unwrap();
        run_migrations_to(&conn, 189).unwrap();
        conn.execute_batch(
            r#"INSERT INTO agents (id, name, description, agent_md, frontmatter) VALUES
                ('off', 'Off', '', '', '{"memory":{"context_isolated":false,"topics":[{"slug":"lead","description":"A lead"}]},"workflows":{}}'),
                ('on', 'Nanna', '', '', '{"memory":{"context_isolated":true,"share_with":["*"]}}'),
                ('named', 'Named', '', '', '{"memory":{"mode":"confidential","context_isolated":false}}'),
                ('none', 'None', '', '', '{"workflows":{}}'),
                ('blank', 'Blank', '', '', ''),
                ('broken', 'Broken', '', '', '{not json');"#,
        )
        .unwrap();

        run_migrations(&conn).unwrap();

        let fm = |id: &str| -> String {
            conn.query_row("SELECT frontmatter FROM agents WHERE id = ?1", [id], |r| r.get(0)).unwrap()
        };
        let memory = |id: &str| -> serde_json::Value { serde_json::from_str::<serde_json::Value>(&fm(id)).unwrap()["memory"].clone() };
        assert_eq!(memory("off")["mode"], "single");
        assert_eq!(memory("off")["topics"][0]["slug"], "lead", "the rest of memory is kept");
        assert_eq!(memory("on")["mode"], "separate");
        assert_eq!(memory("on")["share_with"][0], "*");
        assert_eq!(memory("named")["mode"], "confidential", "a named mode is kept");
        for id in ["off", "on", "named"] {
            assert!(memory(id).get("context_isolated").is_none(), "{id}: the flag goes: {}", fm(id));
        }
        assert_eq!(fm("none"), r#"{"workflows":{}}"#, "no flag, nothing to map");
        assert_eq!(fm("blank"), "");
        assert_eq!(fm("broken"), "{not json", "unreadable frontmatter is left for the reader to fail closed on");

        // Idempotent: the statements, run again, change nothing.
        let all = || -> Vec<String> {
            conn.prepare("SELECT frontmatter FROM agents ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
        };
        let before = all();
        conn.execute_batch(&extract_goose_up(include_str!("../migrations/0190_memory_mode.sql"))).unwrap();
        assert_eq!(all(), before);
    }

    /// A migration file without goose markers is applied verbatim — the
    /// whole file is the Up script, not silently skipped.
    #[test]
    fn extract_goose_up_without_markers_uses_whole_file() {
        let sql = "CREATE TABLE bare (id INT);";
        assert_eq!(extract_goose_up(sql), sql);
    }

    /// At the upgrade, every open ask becomes a wait in the engine with its
    /// first reminder a day out, and the asks the old 72-hour sweep settled
    /// stay settled, as the declines they were. Nothing expires an ask now.
    #[test]
    fn open_asks_become_engine_waits_and_expired_ones_stay_declined() {
        let path = std::env::temp_dir().join(format!("nebo-upgrade-{}.db", uuid::Uuid::new_v4()));
        let conn = Connection::open(&path).unwrap();
        run_migrations_to(&conn, 180).unwrap();
        conn.execute_batch(
            "INSERT INTO permission_asks (id, agent_id, session_key, door, ask_case, sentence, target, call, seat, status, created_at, expires_at)
               VALUES ('open-1', 'emp', 'agent:emp:web', '\"chat\"', '{}', 'texting +15550142', '{}', '{}', '{}', 'open', 100, 259300);
             INSERT INTO permission_asks (id, agent_id, session_key, door, ask_case, sentence, target, call, seat, status, answer, created_at, expires_at, answered_at)
               VALUES ('old-1', 'emp', 'agent:emp:web', '\"chat\"', '{}', 'texting +15550177', '{}', '{}', '{}', 'expired', 'no', 100, 259300, 259300);",
        )
        .unwrap();
        run_migrations_to(&conn, 181).unwrap();
        let columns: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('permission_asks')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(!columns.iter().any(|c| c == "expires_at"), "no expiry: {columns:?}");
        let old: (String, Option<String>) =
            conn.query_row("SELECT status, answer FROM permission_asks WHERE id = 'old-1'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(old, ("answered".to_string(), Some("no".to_string())), "a settled ask stays settled");
        drop(conn);
        let store = crate::Store::new(&path.to_string_lossy()).unwrap();
        let run = store.engine_get_run("open-1").unwrap().expect("the open ask is a run in the engine");
        assert_eq!((run.kind.as_str(), run.state.as_str(), run.agent_id.as_str()), ("ask", "waiting", "emp"));
        let wait = store.engine_get_wait(run.current_wait_id.expect("a live wait")).unwrap().unwrap();
        assert_eq!((wait.action.as_str(), wait.on_kind.as_str(), wait.key.as_str()), ("resume", "answer", "ask:open-1"));
        let due = wait.deadline.expect("the first reminder");
        let now = chrono::Utc::now().timestamp();
        assert!((now + 86_400 - 60..=now + 86_400 + 60).contains(&due), "a day out: {due}");
        let timers = store.engine_pending_timers("wait").unwrap();
        assert_eq!(timers.iter().filter(|e| e.target_id == wait.id.to_string()).count(), 1, "the reminder is the wait's timer");
        assert!(store.engine_get_run("old-1").unwrap().is_none(), "a settled ask waits on nothing");
    }
}
