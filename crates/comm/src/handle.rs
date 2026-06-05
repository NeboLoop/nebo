//! Canonical bot-handle formatting — the SINGLE source of truth on the desktop.
//!
//! This mirrors neboloop's `defaultHandle` / `slugify`
//! (`neboloop/internal/loops/slugify.go`), which is the authority for the
//! `bot_<...>` handle an agent is registered and routed under. Every place that
//! needs the bot's default-agent handle MUST call [`default_bot_handle`] — do
//! NOT open-code `format!("bot_{}", ...)` anywhere else, or the variants drift
//! and identity/routing breaks.

/// Mirror of neboloop `slugify`: lowercase, trim, collapse each run of
/// non-alphanumeric characters to a single `-`, then trim leading/trailing `-`.
///
/// Output is `[a-z0-9-]` only — it can NEVER contain `_`, so a slugified name
/// can never start with `bot_` and masquerade as the primary's handle. Use this
/// for every secondary-agent slug.
pub fn slugify(s: &str) -> String {
    let lower = s.trim().to_lowercase();
    let mut out = String::new();
    let mut prev_dash = false;
    for c in lower.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// The bot's default-agent handle, mirroring neboloop `defaultHandle`.
///
/// - With a non-empty `handle`: `bot_<slugify(handle)>`.
/// - Otherwise (the id-based default): `bot_` + the first 8 chars of the bot
///   UUID string with dashes removed (e.g. `d486d161-…` → `bot_d486d161`).
///
/// Always `bot_`-prefixed, so an agent is unmistakably a bot.
pub fn default_bot_handle(bot_id: &str, handle: &str) -> String {
    let trimmed = handle.trim();
    if !trimmed.is_empty() {
        let slug = slugify(trimmed);
        if !slug.is_empty() {
            return format!("bot_{slug}");
        }
    }
    let id8: String = bot_id.chars().take(8).collect::<String>().replace('-', "");
    format!("bot_{id8}")
}

/// A SECONDARY agent's globally-unique handle: `bot_<id8>_<slugify(name)>`.
///
/// Bot-scoped so two different bots that both load (say) "Chief of Staff" never
/// collide — each gets `bot_<their-id8>_chief-of-staff`. `slugify` never emits
/// `_`, so the single `_` after the id8 unambiguously marks a secondary.
pub fn secondary_handle(bot_id: &str, name: &str) -> String {
    format!("{}_{}", default_bot_handle(bot_id, ""), slugify(name))
}

/// True if `slug` is a PRIMARY (the bot's own) handle: `bot_`-prefixed with no
/// further `_`. Secondaries are `bot_<id8>_<slug>` (a `_` after the id8). Use
/// this everywhere instead of a bare `starts_with("bot_")`, which now also
/// matches secondaries.
pub fn is_primary_handle(slug: &str) -> bool {
    slug.strip_prefix("bot_")
        .map(|rest| !rest.contains('_'))
        .unwrap_or(false)
}

/// If `slug` is a SECONDARY handle (`bot_<id8>_<agentslug>`), return the
/// `<agentslug>` portion (the slugified agent name); otherwise `None`.
pub fn secondary_agent_slug(slug: &str) -> Option<&str> {
    let rest = slug.strip_prefix("bot_")?;
    let idx = rest.find('_')?;
    Some(&rest[idx + 1..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secondary_handle_is_bot_scoped() {
        assert_eq!(
            secondary_handle("d486d161-180f-…", "Chief of Staff"),
            "bot_d486d161_chief-of-staff"
        );
    }

    #[test]
    fn primary_vs_secondary_detection() {
        assert!(is_primary_handle("bot_d486d161"));
        assert!(is_primary_handle("bot_nebo"));
        assert!(!is_primary_handle("bot_d486d161_chief-of-staff"));
        assert!(!is_primary_handle("chief-of-staff"));
        assert_eq!(secondary_agent_slug("bot_d486d161_chief-of-staff"), Some("chief-of-staff"));
        assert_eq!(secondary_agent_slug("bot_d486d161"), None);
        assert_eq!(secondary_agent_slug("chief-of-staff"), None);
    }

    #[test]
    fn id_based_default_matches_neboloop() {
        // neboloop: "bot_" + ReplaceAll(botID.String()[:8], "-", "")
        assert_eq!(
            default_bot_handle("d486d161-180f-4dc7-89fc-62bfdb480f01", ""),
            "bot_d486d161"
        );
    }

    #[test]
    fn custom_handle_is_slugified() {
        assert_eq!(default_bot_handle("d486d161-180f", "Chief of Staff"), "bot_chief-of-staff");
        assert_eq!(default_bot_handle("d486d161-180f", "  Nebo  "), "bot_nebo");
    }

    #[test]
    fn blank_handle_falls_back_to_id() {
        assert_eq!(default_bot_handle("d486d161-180f", "   "), "bot_d486d161");
        assert_eq!(default_bot_handle("d486d161-180f", "!!!"), "bot_d486d161");
    }
}
