//! Where a deleted employee's files go. A delete never removes them on the
//! spot: the folder (an app's page and source with it) is moved whole into
//! the trash, and kept there [`KEEP_DAYS`] days before it is cleared.
//!
//! 2026-10-01: an employee "renamed" an app by deleting it and making a new
//! one, and the app's source went with the delete.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long a deleted employee's files are kept.
pub const KEEP_DAYS: u64 = 30;

/// Move `path` (a folder or a file) into `trash`, under
/// `<unix seconds>-<its name>/<its name>`, and clear what has been there
/// longer than [`KEEP_DAYS`]. Returns where it now is. When it can't be
/// moved, nothing is removed: the error says why and the files stay put.
pub fn discard(path: &Path, trash: &Path) -> std::io::Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no file name"))?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    clear_expired(trash, now);
    let slot = (0..)
        .map(|n| match n {
            0 => trash.join(format!("{now}-{}", name.to_string_lossy())),
            n => trash.join(format!("{now}-{} {n}", name.to_string_lossy())),
        })
        .find(|slot| !slot.exists())
        .expect("an unbounded counter finds a free slot");
    std::fs::create_dir_all(&slot)?;
    let kept = slot.join(name);
    if let Err(e) = std::fs::rename(path, &kept) {
        let _ = std::fs::remove_dir(&slot);
        return Err(e);
    }
    Ok(kept)
}

/// Clear what has been in `trash` longer than [`KEEP_DAYS`], read off the
/// time each slot's name starts with. Anything else there is left alone.
fn clear_expired(trash: &Path, now: u64) {
    let Ok(entries) = std::fs::read_dir(trash) else { return };
    let keep = Duration::from_secs(KEEP_DAYS * 24 * 60 * 60).as_secs();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(at) = name.split_once('-').and_then(|(ts, _)| ts.parse::<u64>().ok()) else { continue };
        if now.saturating_sub(at) > keep && entry.path().is_dir() {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_discarded_folder_is_kept_whole_and_old_ones_are_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("agents").join("Tweet");
        std::fs::create_dir_all(app.join("src")).unwrap();
        std::fs::write(app.join("src").join("App.tsx"), "export default 1").unwrap();
        let trash = dir.path().join("trash");
        let old = trash.join("1000-Gone");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(trash.join("notes")).unwrap();

        let kept = discard(&app, &trash).unwrap();
        assert!(!app.exists());
        assert_eq!(std::fs::read_to_string(kept.join("src").join("App.tsx")).unwrap(), "export default 1");
        assert!(kept.starts_with(&trash) && kept.ends_with("Tweet"), "{}", kept.display());
        assert!(!old.exists(), "older than {KEEP_DAYS} days is cleared");
        assert!(trash.join("notes").exists(), "what isn't a slot is left alone");

        // The same name again gets its own slot.
        std::fs::create_dir_all(&app).unwrap();
        let again = discard(&app, &trash).unwrap();
        assert_ne!(again, kept);
        assert!(kept.exists() && again.exists());
    }

    #[test]
    fn what_cant_be_moved_stays() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("agents").join("Nobody");
        assert!(discard(&missing, &dir.path().join("trash")).is_err());
        assert_eq!(std::fs::read_dir(dir.path().join("trash")).unwrap().count(), 0, "no empty slot left behind");
    }
}
