//! User-supplied path resolution.
//!
//! Two helpers, one fixes one class of bug:
//!
//! - [`expand`] does `~` / `~/...` expansion with no existence check. Use
//!   it when the path is for a file that doesn't exist yet (writes,
//!   `create_dir_all`, etc.).
//! - [`resolve`] expands tilde **and** falls back to a Unicode-whitespace
//!   tolerant directory scan when the literal path doesn't exist. Use it
//!   for reads, uploads, and anywhere the file must already exist.
//!
//! Why the fuzzy fallback: macOS Screenshot files contain a U+202F
//! NARROW NO-BREAK SPACE between the time and "AM"/"PM" — e.g.
//! `Screenshot 2026-05-27 at 10.48.06\u{202F}AM.png`. Users type a
//! regular space when referring to the file in chat, so the LLM
//! constructs paths with U+0020 and `std::fs::open` returns ENOENT
//! against a file that visibly exists in the user's Finder. This isn't
//! Slack-specific or upload-specific — every tool that takes a user
//! path hits it. One resolver, used everywhere.
//!
//! The fallback is **safe by construction**: zero matches → NotFound,
//! exactly one match → use it (and log INFO so we see how often this
//! triggers), more than one → Ambiguous with the candidate list. We
//! never silently pick one of several similar files.

use std::path::{Path, PathBuf};

/// Whitespace code points that visually look like — or are routinely
/// confused with — `U+0020 SPACE`. All of them collapse to a single
/// space when we compare a user-supplied filename to what's on disk.
///
/// This list is intentionally narrow. We DON'T normalize letters,
/// case, NFC vs NFD, accents, or punctuation — those would expand the
/// surface area of false matches dramatically. Only whitespace.
fn is_normalizable_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}' // tab
        | '\u{00A0}' // no-break space
        | '\u{2000}' // en quad
        | '\u{2001}' // em quad
        | '\u{2002}' // en space
        | '\u{2003}' // em space
        | '\u{2004}' // three-per-em space
        | '\u{2005}' // four-per-em space
        | '\u{2006}' // six-per-em space
        | '\u{2007}' // figure space
        | '\u{2008}' // punctuation space
        | '\u{2009}' // thin space
        | '\u{200A}' // hair space
        | '\u{202F}' // narrow no-break space (macOS Screenshot uses this)
        | '\u{205F}' // medium mathematical space
        | '\u{3000}' // ideographic space
    )
}

/// Collapse normalizable whitespace into single U+0020 spaces, so
/// `"foo\u{202F}AM.png"` and `"foo AM.png"` compare equal.
fn normalize_whitespace(s: &str) -> String {
    s.chars()
        .map(|c| if is_normalizable_whitespace(c) { ' ' } else { c })
        .collect()
}

/// Why a path couldn't be resolved.
#[derive(Debug, Clone)]
pub enum ResolveErrorKind {
    /// The literal path didn't exist and no whitespace-normalized match
    /// was found in the parent directory.
    NotFound,
    /// More than one file in the parent directory matched after
    /// whitespace normalization. We refuse to guess which one the
    /// caller meant.
    Ambiguous,
}

/// Path resolution failure, carrying enough context for the agent to
/// either pick the right candidate and retry or surface a clear message
/// to the user.
#[derive(Debug, Clone)]
pub struct ResolveError {
    pub kind: ResolveErrorKind,
    pub input: String,
    pub candidates: Vec<PathBuf>,
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            ResolveErrorKind::NotFound => {
                if self.candidates.is_empty() {
                    write!(f, "file not found: {}", self.input)
                } else {
                    write!(
                        f,
                        "file not found: {} (closest matches in parent dir: {})",
                        self.input,
                        candidate_list(&self.candidates)
                    )
                }
            }
            ResolveErrorKind::Ambiguous => write!(
                f,
                "ambiguous path: {} matches multiple files after whitespace normalization ({}). Use the exact filename.",
                self.input,
                candidate_list(&self.candidates)
            ),
        }
    }
}

impl std::error::Error for ResolveError {}

fn candidate_list(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Expand `~` / `~/...` to the user's home directory. Does NOT check
/// that the resulting path exists. Use for writes / new files.
pub fn expand(input: &str) -> PathBuf {
    if input == "~" {
        if let Some(home) = home_dir() {
            return home;
        }
    } else if let Some(rest) = input.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(input)
}

/// Resolve a user-supplied path to an existing filesystem entry.
///
/// 1. Expand tilde
/// 2. If the literal path exists → return it
/// 3. Else look in the parent directory for an entry whose name matches
///    the requested filename after Unicode whitespace normalization
/// 4. Exactly one match → return its absolute path
/// 5. Zero matches → `ResolveError::NotFound`
/// 6. Multiple matches → `ResolveError::Ambiguous` with the candidate list
pub fn resolve(input: &str) -> Result<PathBuf, ResolveError> {
    let expanded = expand(input);

    if expanded.exists() {
        return Ok(expanded);
    }

    // Try the whitespace-normalized fallback. We need the parent
    // directory and the requested filename to scan.
    let parent = match expanded.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => {
            return Err(ResolveError {
                kind: ResolveErrorKind::NotFound,
                input: input.to_string(),
                candidates: Vec::new(),
            })
        }
    };
    let target = match expanded.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => {
            return Err(ResolveError {
                kind: ResolveErrorKind::NotFound,
                input: input.to_string(),
                candidates: Vec::new(),
            })
        }
    };
    let target_norm = normalize_whitespace(target);

    let entries = match std::fs::read_dir(parent) {
        Ok(e) => e,
        Err(_) => {
            // Parent doesn't exist or we can't read it. No fuzzy match
            // possible.
            return Err(ResolveError {
                kind: ResolveErrorKind::NotFound,
                input: input.to_string(),
                candidates: Vec::new(),
            });
        }
    };

    let mut matches: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let name_os = entry.file_name();
        let name = match name_os.to_str() {
            Some(n) => n,
            None => continue,
        };
        if normalize_whitespace(name) == target_norm {
            matches.push(entry.path());
        }
    }

    match matches.len() {
        1 => {
            let chosen = matches.remove(0);
            tracing::info!(
                input = %input,
                resolved = %chosen.display(),
                "pathres: whitespace-normalized match"
            );
            Ok(chosen)
        }
        0 => Err(ResolveError {
            kind: ResolveErrorKind::NotFound,
            input: input.to_string(),
            candidates: Vec::new(),
        }),
        _ => Err(ResolveError {
            kind: ResolveErrorKind::Ambiguous,
            input: input.to_string(),
            candidates: matches,
        }),
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Borrow-friendly version: resolve a `&Path` that may have been built
/// with whitespace-divergent input. Functionally identical to
/// [`resolve`] for `Path::to_str()`-able paths.
pub fn resolve_path(input: &Path) -> Result<PathBuf, ResolveError> {
    match input.to_str() {
        Some(s) => resolve(s),
        None => Err(ResolveError {
            kind: ResolveErrorKind::NotFound,
            input: input.display().to_string(),
            candidates: Vec::new(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_leaves_absolute_paths_alone() {
        assert_eq!(expand("/tmp/foo"), PathBuf::from("/tmp/foo"));
    }

    #[test]
    fn expand_handles_tilde() {
        let p = expand("~/.config/foo");
        // Should NOT still start with ~
        assert!(!p.to_string_lossy().starts_with('~'));
    }

    #[test]
    fn expand_handles_bare_tilde() {
        let p = expand("~");
        assert!(!p.to_string_lossy().starts_with('~'));
    }

    #[test]
    fn normalize_whitespace_collapses_narrow_no_break_space() {
        // The macOS Screenshot case
        assert_eq!(normalize_whitespace("foo\u{202F}AM.png"), "foo AM.png");
    }

    #[test]
    fn normalize_whitespace_collapses_no_break_space() {
        assert_eq!(normalize_whitespace("a\u{00A0}b"), "a b");
    }

    #[test]
    fn normalize_whitespace_leaves_other_unicode_alone() {
        // accents, emoji, etc. are not normalized
        assert_eq!(normalize_whitespace("café"), "café");
    }

    #[test]
    fn resolve_finds_exact_match() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("hello.txt");
        std::fs::write(&p, b"hi").unwrap();
        let resolved = resolve(p.to_str().unwrap()).unwrap();
        assert_eq!(resolved, p);
    }

    #[test]
    fn resolve_finds_narrow_no_break_space_file() {
        // Simulate the macOS Screenshot case: file on disk has U+202F,
        // caller passes U+0020.
        let dir = tempfile::tempdir().unwrap();
        let real_name = "Screenshot 2026-05-27 at 10.48.06\u{202F}AM.png";
        let real_path = dir.path().join(real_name);
        std::fs::write(&real_path, b"png bytes").unwrap();

        // Caller version: regular space everywhere
        let typed = dir
            .path()
            .join("Screenshot 2026-05-27 at 10.48.06 AM.png");
        let resolved = resolve(typed.to_str().unwrap()).unwrap();
        assert_eq!(resolved, real_path);
    }

    #[test]
    fn resolve_errors_when_nothing_matches() {
        let dir = tempfile::tempdir().unwrap();
        let typed = dir.path().join("nonexistent.png");
        let err = resolve(typed.to_str().unwrap()).unwrap_err();
        assert!(matches!(err.kind, ResolveErrorKind::NotFound));
    }

    #[test]
    fn resolve_errors_on_ambiguous_match() {
        let dir = tempfile::tempdir().unwrap();
        // Two files that normalize to the same string
        std::fs::write(dir.path().join("foo\u{202F}bar.txt"), b"a").unwrap();
        std::fs::write(dir.path().join("foo\u{00A0}bar.txt"), b"b").unwrap();
        let typed = dir.path().join("foo bar.txt");
        let err = resolve(typed.to_str().unwrap()).unwrap_err();
        assert!(matches!(err.kind, ResolveErrorKind::Ambiguous));
        assert_eq!(err.candidates.len(), 2);
    }
}
