//! The rules every brief of code work carries: the project's own (its
//! `CLAUDE.md`, `AGENTS.md`, a code-auditor document), the owner's standing
//! rules for the employee, and the house rules of working on someone's code.

use std::path::Path;

/// The project files whose words are rules for anyone working in it.
pub const RULE_FILES: &[&str] = &["CLAUDE.md", "AGENTS.md"];

/// A code-auditor document, wherever the project keeps it (`docs/`, `docs/sme/`).
pub const AUDITOR_FILE: &str = "CODE_AUDITOR.md";

/// The most of one rules file a brief carries.
pub const FILE_CHARS: usize = 12_000;

/// The house rules: how this work is done on anyone's code.
pub const HOUSE_RULES: &str = "\
- Read-only means read-only: a reviewer or investigator changes nothing, anywhere.
- Never touch the owner's working copy. All work happens in the isolated copy you were given; it is merged back \
only when the work is verified, or when the owner chooses to merge it anyway.
- Format only the lines you changed. Never run a formatter across the whole project.
- Builds use their own build folder, deleted when the work is done.
- Build and run the tests before anything is pushed or merged.
- Make the right fix, never a quick one: no workarounds, no TODOs, no second way of doing what the code already does.
- If another employee or session is working in the same project, message them before touching shared files, and \
stay off their files.";

/// A rules file's text: a regular file only. A symlink is never followed,
/// so a project can't point a brief at a file outside it.
fn read_rules_file(path: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_file() {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// The project's rules files under `root`, each headed by its path, cut to
/// [`FILE_CHARS`]. Empty when the project has none.
pub fn project_rules(root: &Path) -> String {
    let mut found: Vec<(String, String)> = Vec::new();
    for rel in RULE_FILES {
        if let Some(text) = read_rules_file(&root.join(rel)) {
            found.push((rel.to_string(), text));
        }
    }
    for dir in ["", "docs", "docs/sme", "doc"] {
        let rel = if dir.is_empty() {
            AUDITOR_FILE.to_string()
        } else {
            format!("{dir}/{AUDITOR_FILE}")
        };
        if let Some(text) = read_rules_file(&root.join(&rel)) {
            found.push((rel, text));
            break;
        }
    }
    found
        .into_iter()
        .map(|(rel, text)| {
            let text = text.trim();
            let cut: String = text.chars().take(FILE_CHARS).collect();
            let more = if cut.len() < text.len() {
                format!("\n[… the rest of {rel} is in the project]")
            } else {
                String::new()
            };
            format!("### {rel}\n{cut}{more}")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The rules section of a brief: the house rules, the owner's standing
/// rules for this employee (`owner`), and the project's own.
pub fn brief_rules(root: &Path, owner: &str) -> String {
    let mut out = format!("## How this work is done\n{HOUSE_RULES}");
    if !owner.trim().is_empty() {
        out.push_str(&format!(
            "\n\n## The owner's standing rules\n{}",
            owner.trim()
        ));
    }
    let project = project_rules(root);
    if !project.is_empty() {
        out.push_str(&format!("\n\n## The project's rules\n{project}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_brief_carries_the_house_the_owners_and_the_projects_rules() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "Use pnpm only.").unwrap();
        std::fs::create_dir_all(dir.path().join("docs/sme")).unwrap();
        std::fs::write(
            dir.path().join("docs/sme/CODE_AUDITOR.md"),
            "No competing pathways.",
        )
        .unwrap();
        let rules = brief_rules(dir.path(), "Never deploy on Fridays.");
        assert!(rules.contains("Never touch the owner's working copy"));
        assert!(rules.contains("Never deploy on Fridays."));
        assert!(rules.contains("### CLAUDE.md\nUse pnpm only."));
        assert!(rules.contains("### docs/sme/CODE_AUDITOR.md\nNo competing pathways."));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_rules_file_is_never_followed() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "SECRET").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            dir.path().join("CLAUDE.md"),
        )
        .unwrap();
        assert_eq!(project_rules(dir.path()), "");
    }

    #[test]
    fn a_project_without_rules_files_adds_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(project_rules(dir.path()), "");
        assert!(!brief_rules(dir.path(), "").contains("project's rules"));
    }
}
