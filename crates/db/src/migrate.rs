use rust_embed::Embed;
use rusqlite::Connection;
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
    <Migrations as Embed>::iter().map(|f| f.to_string()).collect()
}

fn get_file(name: &str) -> Option<EmbeddedFile> {
    <Migrations as Embed>::get(name)
}

/// Run all pending migrations on the database connection.
/// Compatible with goose's migration tracking (goose_db_version table).
pub fn run_migrations(conn: &Connection) -> Result<(), NeboError> {
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

    let mut applied_count = 0;

    for filename in &migration_files {
        // Extract version number from filename (e.g., "0001_initial_schema.sql" -> 1)
        let version = extract_version(filename).ok_or_else(|| {
            NeboError::Migration(format!("invalid migration filename: {filename}"))
        })?;

        if applied.contains(&version) {
            continue;
        }

        // Read migration SQL
        let data = get_file(filename).ok_or_else(|| {
            NeboError::Migration(format!("migration file not found: {filename}"))
        })?;
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
    filename
        .split('_')
        .next()?
        .parse::<i64>()
        .ok()
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
