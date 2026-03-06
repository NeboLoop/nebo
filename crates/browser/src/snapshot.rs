use std::collections::HashMap;

use crate::actions::INTERACTIVE_ROLES;
use crate::snapshot_store::AnnotatedElement;
use crate::ElementRef;

/// Options for accessibility tree snapshots.
pub struct SnapshotOptions {
    pub include_refs: bool,
}

impl Default for SnapshotOptions {
    fn default() -> Self {
        Self { include_refs: true }
    }
}

/// Annotate an accessibility tree with element references.
/// Takes a raw ARIA snapshot string and adds [eN] refs to interactive elements.
pub fn annotate_snapshot(snapshot: &str, include_refs: bool) -> (String, Vec<ElementRef>) {
    if !include_refs {
        return (snapshot.to_string(), vec![]);
    }

    let mut refs = Vec::new();
    let mut output = String::new();
    let mut ref_counter = 1;

    for line in snapshot.lines() {
        let trimmed = line.trim_start_matches(|c: char| c == '-' || c == ' ');

        // Check if line starts with an interactive role
        let is_interactive = INTERACTIVE_ROLES.iter().any(|&role| {
            trimmed.starts_with(role)
                && (trimmed.len() == role.len()
                    || trimmed[role.len()..].starts_with(|c: char| c == ' ' || c == '"'))
        });

        if is_interactive {
            let ref_id = format!("e{}", ref_counter);
            ref_counter += 1;

            // Extract role and name
            let (role, name) = parse_role_name(trimmed);

            // Build selector
            let selector = if name.is_empty() {
                format!("role={}", role)
            } else {
                format!("role={}[name=\"{}\"]", role, name)
            };

            refs.push(ElementRef {
                id: ref_id.clone(),
                role: role.to_string(),
                name: name.to_string(),
                selector,
            });

            output.push_str(line);
            output.push_str(&format!(" [{}]", ref_id));
        } else {
            output.push_str(line);
        }
        output.push('\n');
    }

    (output, refs)
}

/// Parse "button \"Submit\"" into ("button", "Submit").
fn parse_role_name(s: &str) -> (&str, &str) {
    if let Some(quote_start) = s.find('"') {
        let role = s[..quote_start].trim();
        let rest = &s[quote_start + 1..];
        if let Some(quote_end) = rest.find('"') {
            return (role, &rest[..quote_end]);
        }
        return (role, "");
    }

    // No quotes — role only
    let role = s.split_whitespace().next().unwrap_or(s);
    (role, "")
}

/// Role prefix map for role-based element IDs.
fn role_prefix(role: &str) -> &str {
    match role {
        "button" => "B",
        "textbox" | "textarea" | "searchbox" => "T",
        "link" => "L",
        "checkbox" => "C",
        "menuitem" => "M",
        "slider" | "spinbutton" => "S",
        "tab" => "A",
        "radio" => "R",
        "combobox" | "listbox" => "D",
        "switch" => "W",
        _ => "E", // generic
    }
}

/// Annotate an accessibility tree with role-based element IDs (B1, T2, L3, etc.).
/// Returns the annotated text and a list of AnnotatedElement with role-based IDs.
pub fn annotate_with_role_ids(snapshot: &str) -> (String, Vec<AnnotatedElement>) {
    let mut elements = Vec::new();
    let mut output = String::new();
    let mut counters: HashMap<String, usize> = HashMap::new();

    for line in snapshot.lines() {
        let trimmed = line.trim_start_matches(|c: char| c == '-' || c == ' ');

        let is_interactive = INTERACTIVE_ROLES.iter().any(|&role| {
            trimmed.starts_with(role)
                && (trimmed.len() == role.len()
                    || trimmed[role.len()..].starts_with(|c: char| c == ' ' || c == '"'))
        });

        if is_interactive {
            let (role, name) = parse_role_name(trimmed);
            let prefix = role_prefix(role);
            let counter = counters.entry(prefix.to_string()).or_insert(0);
            *counter += 1;
            let element_id = format!("{}{}", prefix, counter);

            let label = if name.len() > 40 {
                format!("{}...", &name[..37])
            } else {
                name.to_string()
            };

            let selector = if name.is_empty() {
                format!("role={}", role)
            } else {
                format!("role={}[name=\"{}\"]", role, name)
            };

            elements.push(AnnotatedElement {
                id: element_id.clone(),
                role: role.to_string(),
                label,
                bounds: None,
                actionable: true,
                selector,
            });

            output.push_str(line);
            output.push_str(&format!(" [{}]", element_id));
        } else {
            output.push_str(line);
        }
        output.push('\n');
    }

    (output, elements)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_annotate_snapshot() {
        let snapshot = r#"- button "Submit"
  - text "Click me"
- textbox
- link "Home"
- heading "Title""#;

        let (annotated, refs) = annotate_snapshot(snapshot, true);

        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0].id, "e1");
        assert_eq!(refs[0].role, "button");
        assert_eq!(refs[0].name, "Submit");
        assert_eq!(refs[1].id, "e2");
        assert_eq!(refs[1].role, "textbox");
        assert_eq!(refs[2].id, "e3");
        assert_eq!(refs[2].role, "link");

        assert!(annotated.contains("[e1]"));
        assert!(annotated.contains("[e2]"));
        assert!(annotated.contains("[e3]"));
        // heading is not interactive
        assert!(!annotated.contains("[e4]"));
    }

    #[test]
    fn test_parse_role_name() {
        assert_eq!(parse_role_name("button \"OK\""), ("button", "OK"));
        assert_eq!(parse_role_name("textbox"), ("textbox", ""));
        assert_eq!(parse_role_name("link \"Home page\""), ("link", "Home page"));
    }

    #[test]
    fn test_annotate_with_role_ids() {
        let snapshot = r#"- button "Submit"
  - text "Click me"
- textbox
- link "Home"
- button "Cancel"
- heading "Title""#;

        let (annotated, elements) = annotate_with_role_ids(snapshot);

        assert_eq!(elements.len(), 4);
        // Buttons: B1, B2
        assert_eq!(elements[0].id, "B1");
        assert_eq!(elements[0].role, "button");
        assert_eq!(elements[0].label, "Submit");
        assert_eq!(elements[3].id, "B2");
        assert_eq!(elements[3].role, "button");
        assert_eq!(elements[3].label, "Cancel");
        // Textbox: T1
        assert_eq!(elements[1].id, "T1");
        assert_eq!(elements[1].role, "textbox");
        // Link: L1
        assert_eq!(elements[2].id, "L1");
        assert_eq!(elements[2].role, "link");
        assert_eq!(elements[2].label, "Home");

        assert!(annotated.contains("[B1]"));
        assert!(annotated.contains("[T1]"));
        assert!(annotated.contains("[L1]"));
        assert!(annotated.contains("[B2]"));
        // heading is not interactive
        assert!(!annotated.contains("[E1]"));
    }

    #[test]
    fn test_role_prefixes() {
        assert_eq!(role_prefix("button"), "B");
        assert_eq!(role_prefix("textbox"), "T");
        assert_eq!(role_prefix("link"), "L");
        assert_eq!(role_prefix("checkbox"), "C");
        assert_eq!(role_prefix("menuitem"), "M");
        assert_eq!(role_prefix("tab"), "A");
        assert_eq!(role_prefix("radio"), "R");
        assert_eq!(role_prefix("unknown"), "E");
    }

    #[test]
    fn test_long_label_truncation() {
        let long_name = "A".repeat(50);
        let snapshot = format!("- button \"{}\"", long_name);
        let (_, elements) = annotate_with_role_ids(&snapshot);
        assert_eq!(elements.len(), 1);
        assert!(elements[0].label.len() <= 40);
        assert!(elements[0].label.ends_with("..."));
    }
}
