//! The migrations run forward on a real database.

use super::*;

/// The head this branch migrates to — read from the embedded migrations, so
/// adding one never leaves this proof asserting a stale number.
fn head() -> i64 {
    db::migrate::head_version()
}

fn query_i64(path: &Path, sql: &str) -> i64 {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap()
}

/// Whether a table has a column yet. The synthetic source starts at 0152,
/// before the staffing columns exist, and a column that is not there holds
/// nothing — which is the true count, not a swallowed error.
fn has_column(path: &Path, table: &str, column: &str) -> bool {
    let conn = rusqlite::Connection::open(path).unwrap();
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})")).unwrap();
    let mut rows = stmt.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        if row.get::<_, String>(1).unwrap() == column {
            return true;
        }
    }
    false
}

/// Copy a SQLite database (and its WAL and shm, when present) read-only into
/// `dir`, never opening the original.
fn copy_db(src: &Path, dir: &Path) -> PathBuf {
    let dst = dir.join("nebo.db");
    std::fs::copy(src, &dst).unwrap();
    for suffix in ["-wal", "-shm"] {
        let side = PathBuf::from(format!("{}{suffix}", src.display()));
        if side.is_file() {
            std::fs::copy(&side, dir.join(format!("nebo.db{suffix}"))).unwrap();
        }
    }
    dst
}

/// The databases on this machine worth migrating: the live one and the
/// backups the migrator itself left before earlier heads. Absent in CI.
fn real_databases() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else { return Vec::new() };
    let data = home.join("Library").join("Application Support").join("Nebo").join("data");
    ["nebo.db", "nebo.db.pre-v0152.bak", "nebo.db.pre-v0155.bak"]
        .iter()
        .map(|n| data.join(n))
        .filter(|p| p.is_file())
        .collect()
}

/// A database in the shape the last release left it (migration 0152) with
/// a few employees on it, one carrying a package's department — what an
/// upgrade meets when no real database is on the machine.
fn database_at_0152(dir: &Path) -> PathBuf {
    let path = dir.join("nebo.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    db::migrate::run_migrations_to(&conn, 152).unwrap();
    conn.execute_batch(
        "INSERT INTO agents (id, kind, name, description, agent_md, frontmatter) VALUES
            ('assistant', 'user', 'Nebo', '', '', ''),
            ('bk', 'installed', 'Bookkeeper', '', '', '{}'),
            ('om', 'user', 'Office Manager', '', '', '{}');
         UPDATE agents SET department = 'accounting' WHERE id = 'bk';",
    )
    .unwrap();
    path
}

/// The migrator reaches head on a real database copied read-only from this
/// machine — the live one and the pre-0152 and pre-0155 backups where they
/// exist — and on a database built at 0152 when none is. No existing agent
/// gains a department lock or a reporting line; a department a package
/// wrote stays, unlocked; the new tables exist; running again changes
/// nothing.
#[test]
fn migrations_run_forward_on_a_real_database() {
    let tmp = tempfile::tempdir().unwrap();
    let mut sources: Vec<(String, PathBuf)> = Vec::new();
    for (i, real) in real_databases().into_iter().enumerate() {
        let dir = tmp.path().join(format!("real-{i}"));
        std::fs::create_dir_all(&dir).unwrap();
        sources.push((real.display().to_string(), copy_db(&real, &dir)));
    }
    let synth = tmp.path().join("synthetic");
    std::fs::create_dir_all(&synth).unwrap();
    sources.push(("a database built at 0152".to_string(), database_at_0152(&synth)));

    for (label, path) in &sources {
        let before = query_i64(path, "SELECT COALESCE(MAX(version), 0) FROM _nebo_migrations");
        let agents_before = query_i64(path, "SELECT COUNT(*) FROM agents");
        let with_department = query_i64(path, "SELECT COUNT(*) FROM agents WHERE department IS NOT NULL AND department != ''");
        // What the database already had. A real machine's employees can carry
        // a lock or a reporting line the owner set while using Nebo, and this
        // proof is about what the MIGRATOR does, not about finding a database
        // that happens to be empty of both.
        let locked_before = if has_column(path, "agents", "department_locked") {
            query_i64(path, "SELECT COUNT(*) FROM agents WHERE department_locked != 0")
        } else {
            0
        };
        let reporting_before = if has_column(path, "agents", "reports_to") {
            query_i64(path, "SELECT COUNT(*) FROM agents WHERE reports_to IS NOT NULL")
        } else {
            0
        };
        assert!(agents_before > 0, "{label}: a database with employees on it");

        // The migrator: opening the store runs every pending migration.
        let store = db::Store::new(&path.to_string_lossy()).unwrap_or_else(|e| panic!("{label}: {e}"));
        let reached = query_i64(path, "SELECT MAX(version) FROM _nebo_migrations");
        assert_eq!(reached, head(), "{label}: from {before} to head");
        assert!(before <= head(), "{label}: a database ahead of this branch");
        for v in 153..=head() {
            assert_eq!(query_i64(path, &format!("SELECT COUNT(*) FROM _nebo_migrations WHERE version = {v}")), 1, "{label}: {v} applied");
        }
        // Nothing about an existing employee was invented.
        assert_eq!(query_i64(path, "SELECT COUNT(*) FROM agents"), agents_before, "{label}");
        assert_eq!(query_i64(path, "SELECT COUNT(*) FROM agents WHERE department_locked != 0"), locked_before, "{label}: no agent gained a department lock");
        assert_eq!(query_i64(path, "SELECT COUNT(*) FROM agents WHERE reports_to IS NOT NULL"), reporting_before, "{label}: no agent gained a reporting line");
        assert_eq!(
            query_i64(path, "SELECT COUNT(*) FROM agents WHERE department IS NOT NULL AND department != ''"),
            with_department,
            "{label}: a department a package wrote stays"
        );
        // The rows read back through the model that grew the columns.
        let locked_rows = store
            .list_agents(10_000, 0)
            .unwrap()
            .iter()
            .filter(|a| a.department_locked != 0)
            .count() as i64;
        let reporting_rows = store
            .list_agents(10_000, 0)
            .unwrap()
            .iter()
            .filter(|a| a.reports_to.is_some())
            .count() as i64;
        assert_eq!(locked_rows, locked_before, "{label}: the model reads back what the table holds");
        assert_eq!(reporting_rows, reporting_before, "{label}: the model reads back what the table holds");
        // The new tables exist and are empty of anything invented.
        assert_eq!(query_i64(path, "SELECT COUNT(*) FROM company_policy"), 0, "{label}");
        assert_eq!(query_i64(path, "SELECT COUNT(*) FROM operation_counters"), 0, "{label}");
        assert_eq!(query_i64(path, "SELECT COUNT(*) FROM assignments"), 0, "{label}");
        assert_eq!(query_i64(path, "SELECT COUNT(*) FROM pragma_table_info('agents') WHERE name = 'context_section'"), 0, "{label}: 0156 dropped the column");
        assert_eq!(query_i64(path, "SELECT COUNT(*) FROM pragma_table_info('agents') WHERE name IN ('context_stamp', 'reports_to', 'department_locked')"), 3, "{label}");
        drop(store);
        // Idempotent: opening again applies nothing.
        let applied = query_i64(path, "SELECT COUNT(*) FROM _nebo_migrations");
        let _again = db::Store::new(&path.to_string_lossy()).unwrap();
        assert_eq!(query_i64(path, "SELECT COUNT(*) FROM _nebo_migrations"), applied, "{label}");
    }
    assert!(sources.len() >= 1);
}
