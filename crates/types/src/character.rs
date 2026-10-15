//! The mountain family: which character an employee wears.
//!
//! Every employee is a mountain shape with a face (eyes that blink and a small smile), shaded from its colour. The
//! primary is the assistant, a character of its own in brand blue that no
//! employee wears. The shape is decided HERE and nowhere else: the roster
//! sends the resolved shape, and the web app and the mobile app only draw it
//! (their path data lives in `app/src/lib/characters.ts` and the mobile
//! app's `lib/theme/characters.dart`, the same values in both).
//!
//! A shape the owner picked always wins. An employee without one gets a
//! stable shape from its role (its name, then its department, then its
//! description), and when nothing in those names a role, from its id.

/// The primary's id, and the character only it wears.
pub const ASSISTANT: &str = "assistant";

/// The shapes an employee can wear, in the order the id fallback walks.
pub const EMPLOYEE_SHAPES: &[&str] = &[
    "ridge",
    "twin",
    "mesa",
    "butte",
    "diamond",
    "dome",
    "snowcap",
    "volcano",
    "sawtooth",
    "hook",
    "flattop",
    "littlehill",
];

/// Words that name a role, and the shape that role wears. Read in order:
/// the first role a word starts with wins.
const ROLES: &[(&[&str], &str)] = &[
    (&["store", "shop", "ecommerce", "e-commerce", "inventory", "retail", "order"], "ridge"),
    (&["content", "marketing", "social", "video", "creator", "brand", "seo", "media"], "volcano"),
    (&["client", "account", "finance", "invoice", "billing", "bookkeep", "payroll", "tax"], "mesa"),
    (&["chief", "staff", "operation", "ops", "team", "project", "standup"], "butte"),
    (&["sales", "lead", "growth", "prospect", "deal"], "twin"),
    (&["research", "analyst", "analytics", "data", "insight"], "diamond"),
    (&["home", "family", "support", "customer", "care", "reception", "concierge"], "dome"),
    (&["legal", "law", "lawyer", "compliance", "contract", "paralegal"], "flattop"),
    (&["engineer", "developer", "code", "coding", "devops", "tech", "it"], "sawtooth"),
    (&["writer", "writing", "editor", "copy", "blog", "newsletter"], "hook"),
    (&["assistant", "admin", "office", "schedul", "calendar", "inbox", "email"], "dome"),
    (&["recruit", "hiring", "hr", "people", "talent"], "snowcap"),
    (&["design", "creative", "art", "photo"], "littlehill"),
];

/// Whether `shape` is one an employee may wear (the assistant's is not).
pub fn is_employee_shape(shape: &str) -> bool {
    EMPLOYEE_SHAPES.contains(&shape)
}

/// The shape a role named in `text` wears, if any word in it names one.
fn shape_for_role(text: &str) -> Option<&'static str> {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric() && c != '-')
        .filter(|w| !w.is_empty())
        .collect();
    ROLES.iter().find_map(|(keys, shape)| {
        words
            .iter()
            .any(|w| keys.iter().any(|k| if k.len() <= 3 { w == k } else { w.starts_with(k) }))
            .then_some(*shape)
    })
}

/// The shape for an id: the same 31-multiplier hash the web uses for an
/// employee's fallback colour, over the employee shapes.
fn shape_for_id(id: &str) -> &'static str {
    let h = id
        .chars()
        .fold(0u32, |h, c| h.wrapping_mul(31).wrapping_add(c as u32));
    EMPLOYEE_SHAPES[h as usize % EMPLOYEE_SHAPES.len()]
}

/// The character an employee wears: the assistant's for the primary; the
/// owner's pick when it is one; else its role's; else its id's.
pub fn shape_of(
    id: &str,
    stored: Option<&str>,
    name: &str,
    department: Option<&str>,
    description: &str,
) -> &'static str {
    if id == ASSISTANT {
        return ASSISTANT;
    }
    if let Some(s) = stored.and_then(|s| EMPLOYEE_SHAPES.iter().find(|e| **e == s)) {
        return s;
    }
    shape_for_role(name)
        .or_else(|| department.and_then(shape_for_role))
        .or_else(|| shape_for_role(description))
        .unwrap_or_else(|| shape_for_id(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_primary_is_always_the_assistant() {
        assert_eq!(shape_of("assistant", Some("ridge"), "Store Manager", None, ""), "assistant");
    }

    #[test]
    fn the_owners_pick_wins_and_the_assistant_is_not_a_pick() {
        assert_eq!(shape_of("e1", Some("hook"), "Store Manager", None, ""), "hook");
        // The spire left the family: a pick of it goes back to the role's.
        assert_eq!(shape_of("e1", Some("spire"), "Store Manager", None, ""), "ridge");
        assert_eq!(shape_of("e1", Some("assistant"), "Store Manager", None, ""), "ridge");
        assert_eq!(shape_of("e1", Some("nonsense"), "Store Manager", None, ""), "ridge");
        assert!(!is_employee_shape("assistant"));
    }

    #[test]
    fn a_role_decides_the_shape() {
        let s = |name: &str| shape_of("x", None, name, None, "");
        assert_eq!(s("Store Manager"), "ridge");
        assert_eq!(s("Ecommerce Store Manager"), "ridge");
        assert_eq!(s("Content Strategist"), "volcano");
        assert_eq!(s("Client Account Manager"), "mesa");
        assert_eq!(s("Chief of Staff"), "butte");
        assert_eq!(s("Home Manager"), "dome");
        assert_eq!(s("Sales Rep"), "twin");
        assert_eq!(s("Research Analyst"), "diamond");
        // The name speaks first, then the department, then the description.
        assert_eq!(shape_of("x", None, "Piper", Some("sales"), "Writes posts"), "twin");
        assert_eq!(shape_of("x", None, "Piper", None, "Writes the weekly newsletter"), "hook");
    }

    #[test]
    fn without_a_role_the_id_decides_and_it_is_stable() {
        let a = shape_of("8c1f0e2a-77", None, "Piper", None, "");
        for _ in 0..3 {
            assert_eq!(shape_of("8c1f0e2a-77", None, "Piper", None, ""), a);
        }
        assert!(is_employee_shape(a));
        // Pinned: the web and the mobile app read these from the roster, so a
        // change here changes every existing employee's character.
        assert_eq!(shape_for_id(""), "ridge");
        assert_eq!(shape_for_id("a"), "twin");
        assert_eq!(shape_for_id("zz"), "diamond");
    }

    #[test]
    fn short_keys_match_whole_words_only() {
        // "it" must not make "Kitchen Manager" an engineer.
        assert_ne!(shape_for_role("Kitchen Manager"), Some("sawtooth"));
        assert_eq!(shape_for_role("IT Helpdesk"), Some("sawtooth"));
    }
}
