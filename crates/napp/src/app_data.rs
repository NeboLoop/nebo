//! Where a sidecar keeps its data: its `NEBO_DATA_DIR`, working directory and
//! `sidecar.log`, under `<home>/appdata/`, keyed by the artifact's own
//! identity — never by the folder its code happens to sit in.
//!
//! - An app: `appdata/agents/<agent id>/`. The agent id is the manifest `id`
//!   (a marketplace app's artifact id, a user app's minted id), which survives
//!   reinstalls and renames of the app's folder.
//! - A tool from the tool registry: `appdata/plugins/<slug>/`, the name of the
//!   folder that names the tool, the way plugins key theirs.
//!
//! The data dir used to be named after the code folder's PARENT, so every app
//! under `user/agents/<Name>/` shared `appdata/plugins/agents/` (and every
//! loose tool `appdata/plugins/tools/`). The server's one-time migration
//! (`migration::migrate_app_data_per_app`) moved that data out.

use std::path::{Component, Path, PathBuf};

/// Whose data a data dir holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataKind {
    /// An app, keyed by its agent id.
    App,
    /// A tool-registry tool, keyed by its slug.
    Tool,
}

/// The data dir of the artifact `key` of `kind`. `None` when the key is not a
/// single folder name (empty, a path separator, `.` or `..`), which would put
/// the data outside its `appdata/` folder.
pub fn data_dir(home: &Path, kind: DataKind, key: &str) -> Option<PathBuf> {
    let mut parts = Path::new(key).components();
    let one_name = matches!((parts.next(), parts.next()), (Some(Component::Normal(name)), None) if name == key);
    let folder = match kind {
        DataKind::App => "agents",
        DataKind::Tool => "plugins",
    };
    one_name.then(|| home.join("appdata").join(folder).join(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_must_be_one_folder_name() {
        let home = Path::new("/n");
        assert_eq!(
            data_dir(home, DataKind::App, "neighbor-mail"),
            Some(PathBuf::from("/n/appdata/agents/neighbor-mail"))
        );
        assert_eq!(data_dir(home, DataKind::Tool, "crm"), Some(PathBuf::from("/n/appdata/plugins/crm")));
        for bad in ["", ".", "..", "a/b", "../x", "a/", "/abs"] {
            assert_eq!(data_dir(home, DataKind::App, bad), None, "{bad:?}");
        }
    }
}
