//! One pathway for "a filesystem walk is bounded in scope and time".
//!
//! Two callers share it: the file search in `spotlight_tool`, which walks with
//! `find`, and the file glob in `file_tool`, which walks in process with
//! `walkdir`. Both were found walking the whole disk — the search as
//! `find / -maxdepth 5` (gate 35522052383), the glob as `**/image.png` from
//! `/` (gate 35577273218) — and both ran until the harness ended the run.
//! The scope table, the deadline and the sentence a cut-short walk gives back
//! live here so there is one of each, not one per tool.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long a filesystem walk may run before it gives up.
///
/// The runner's per-tool budget is 300 s and the harness ends a silent run at
/// 180 s, so neither of those can ever be the thing that stops a walk: the run
/// is over before the tool says a word. A walk that has not found the file in
/// twenty seconds is walking the wrong place, and the employee is better
/// served by a sentence telling it to narrow the search than by three more
/// minutes of walking the disk.
///
/// This is a whole call's budget, not one command's: when plocate is absent
/// and the find fallback runs, both share these twenty seconds.
pub(crate) const WALK_DEADLINE: Duration = Duration::from_secs(20);

/// The engine budget a walking tool asks for: the walk's own deadline plus
/// room to format the answer. The tool always stops itself first, so the model
/// reads the walk's own sentence about narrowing the query instead of the
/// runner's generic timeout text.
pub(crate) const WALK_EXECUTION_TIMEOUT: Duration = Duration::from_secs(30);

/// How many entries an in-process walk may visit. `find` is bounded by depth
/// instead; `walkdir` has no depth to lean on when the pattern is recursive,
/// so it counts.
pub(crate) const MAX_WALK_ENTRIES: usize = 200_000;

/// Paths a walk must never enter: kernel and device trees that are not files,
/// the runtime churn beside them, and the autofs triggers that mount a remote
/// filesystem merely by being looked at. Pruned only when they are actually
/// under the root, so a scoped walk pays nothing for them.
const NEVER_WALK: [&str; 11] = [
    "/proc",
    "/sys",
    "/dev",
    "/run",
    "/var/run",
    "/mnt",
    "/media",
    "/net",
    "/private/var/vm",
    "/System/Volumes/Data",
    "/Volumes",
];

/// Filesystem types a walk must never enter. A read of a directory on one of
/// these can block in the kernel with no timeout and no signal: the gate's
/// orphaned `find` processes were stuck in uninterruptible sleep inside
/// `fuse_readdir` on a virtiofs mount, where neither a deadline, a `kill`, nor
/// `kill_on_drop` can reach them. The only fix that works is not going in.
/// Matched against `/proc/self/mounts`; anything starting with `fuse` counts.
/// The table is Linux-shaped, so only a Linux build reads it — macOS keeps the
/// autofs and removable-volume roots in `NEVER_WALK` instead.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const FOREIGN_FS: [&str; 17] = [
    "virtiofs",
    "nfs",
    "nfs4",
    "cifs",
    "smbfs",
    "smb3",
    "afpfs",
    "autofs",
    "9p",
    "sshfs",
    "davfs",
    "webdav",
    "ceph",
    "glusterfs",
    "lustre",
    "afs",
    "coda",
];

/// Mount points on a filesystem a walk must not enter, read from a
/// `/proc/self/mounts`-shaped table: `device mountpoint fstype options…`,
/// with the kernel's octal escapes for the awkward characters in a path.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
pub(crate) fn parse_foreign_mounts(table: &str) -> Vec<PathBuf> {
    table
        .lines()
        .filter_map(|line| {
            let mut f = line.split_whitespace();
            let _device = f.next()?;
            let point = f.next()?;
            let fstype = f.next()?;
            let foreign = fstype.starts_with("fuse") || FOREIGN_FS.contains(&fstype);
            foreign.then(|| PathBuf::from(unescape_mount_path(point)))
        })
        .collect()
}

/// `/proc/self/mounts` escapes space, tab, newline and backslash in octal.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn unescape_mount_path(raw: &str) -> String {
    if !raw.contains('\\') {
        return raw.to_string();
    }
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            if let Some(c) = std::str::from_utf8(&bytes[i + 1..i + 4])
                .ok()
                .and_then(|o| u8::from_str_radix(o, 8).ok())
            {
                out.push(c as char);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Every mount on this box a walk must not enter. Linux reads the live table;
/// macOS has no `/proc`, and the autofs and removable-volume roots that would
/// hang a walk there are already in `NEVER_WALK`.
fn foreign_mounts() -> Vec<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/self/mounts")
            .map(|t| parse_foreign_mounts(&t))
            .unwrap_or_default()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

/// The paths a walk of `root` must step around: the kernel and device trees,
/// plus every foreign mount, narrowed to the ones actually under the root.
pub(crate) fn skip_paths(root: &Path) -> Vec<PathBuf> {
    let under = |p: &Path| p.starts_with(root) && p != root;
    NEVER_WALK
        .iter()
        .map(PathBuf::from)
        .chain(foreign_mounts())
        .filter(|p| under(p))
        .collect()
}

/// The scope and the clock an in-process walk runs under. `find` takes the
/// same `skip` list as `-path … -prune` arguments and the same deadline as the
/// moment it is killed; a `walkdir` caller carries the whole thing.
pub(crate) struct WalkBounds {
    /// Directories this walk must not enter.
    pub skip: Vec<PathBuf>,
    /// Entries this walk may visit before it stops.
    pub max_entries: usize,
    /// The moment this walk stops, found or not.
    pub deadline: Instant,
}

impl WalkBounds {
    /// What a walk of `root` gets when nobody says otherwise.
    pub fn for_root(root: &Path) -> Self {
        Self {
            skip: skip_paths(root),
            max_entries: MAX_WALK_ENTRIES,
            deadline: Instant::now() + WALK_DEADLINE,
        }
    }

    /// Whether the walk must step around this directory.
    pub fn skips(&self, path: &Path) -> bool {
        self.skip.iter().any(|p| p == path)
    }

    /// Whether the budget is gone after `visited` entries, and which bound
    /// ran out. The clock is read once every 512 entries: a `walkdir` step is
    /// cheap enough that reading it every time would cost more than the walk.
    pub fn spent(&self, visited: usize) -> Option<CutShort> {
        if visited > self.max_entries {
            return Some(CutShort::Entries(self.max_entries));
        }
        if visited % 512 == 0 && Instant::now() > self.deadline {
            return Some(CutShort::Deadline(WALK_DEADLINE));
        }
        None
    }
}

/// Why a walk stopped before it had covered everything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CutShort {
    /// The clock ran out.
    Deadline(Duration),
    /// The walk had visited every entry it was allowed.
    Entries(usize),
}

impl CutShort {
    /// The half-sentence that says which bound ran out, in the tense of
    /// "the walk …".
    fn ran_out(&self) -> String {
        match self {
            CutShort::Deadline(d) => format!("took longer than {} seconds", d.as_secs()),
            CutShort::Entries(n) => format!("passed the {n} entries it may visit"),
        }
    }
}

/// The plain sentence a walk that ran out of budget gives back: what was
/// walked, which bound stopped it, that a miss is therefore not proof of
/// absence, how to narrow it, and whatever it had already found. `what` names
/// the walk ("search for \"budget\"", "glob of \"**/*.png\""); `narrow_with`
/// names the parameter that narrows it.
pub(crate) fn took_too_long(
    what: &str,
    root: &Path,
    cut: CutShort,
    narrow_with: &str,
    partial: &[String],
) -> String {
    let mut msg = format!(
        "The {} under {} {}, so it was stopped before it covered everything: a miss here is \
         not proof of absence. Narrow it and try again: pass {} with the folder the file is \
         likely in, or give a more specific name.",
        what,
        root.display(),
        cut.ran_out(),
        narrow_with
    );
    if !partial.is_empty() {
        msg.push_str(&format!(
            "\n\nWhat it had found before it stopped ({}):\n{}",
            partial.len(),
            partial.join("\n")
        ));
    }
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mounts table, read the way the kernel writes it: only the
    /// filesystems a walk must not enter come back, escapes and all.
    #[test]
    fn the_mounts_table_names_only_the_filesystems_to_stay_out_of() {
        let table = "\
/dev/vda1 / ext4 rw,relatime 0 0
proc /proc proc rw,nosuid 0 0
mount0 /mnt/lima-rosetta fuse.virtiofs rw,nosuid,nodev 0 0
share /Users/stadium virtiofs rw,relatime 0 0
fileserver:/vol /srv/files nfs4 rw 0 0
//host/share /srv/win\\040share cifs rw 0 0
tmpfs /tmp tmpfs rw 0 0
";
        let foreign = parse_foreign_mounts(table);
        assert_eq!(
            foreign,
            vec![
                PathBuf::from("/mnt/lima-rosetta"),
                PathBuf::from("/Users/stadium"),
                PathBuf::from("/srv/files"),
                PathBuf::from("/srv/win share"),
            ],
            "ext4, proc and tmpfs are walkable; fuse, virtiofs, nfs and cifs are not"
        );
    }
}
