//! Intelligence packs: the AI an employee runs on, bundled and assignable.
//!
//! A pack maps each Effort level (`Instant < Low < Medium < High < Max`) to a
//! model, and names the models its Vision and Voice capabilities use. The
//! built-in **Nebo AI** pack runs through Janus and is the only one with
//! Auto: Janus picks the level (owner, 2026-10-07: Auto exists only in
//! Janus). A pack the owner makes brings his own AI (his keys, an agent on
//! his computer) and runs at the level he set.
//!
//! A pack is chosen through the same model string every assignment already
//! carries (the employee's preference, a chat's override, the bot's
//! default): `pack/<id>` or `pack/<id>/<level>`. It resolves to one model at
//! the turn's start and stays fixed for the turn.
//!
//! Design: neboloop `docs/prd/intelligence-packs.md`.

use serde::{Deserialize, Serialize};

/// How much work a turn gets, lowest first: the derived order is the ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Instant,
    Low,
    Medium,
    High,
    Max,
}

impl Effort {
    /// The ladder, lowest first.
    pub const ALL: [Effort; 5] = [Effort::Instant, Effort::Low, Effort::Medium, Effort::High, Effort::Max];

    pub fn as_str(self) -> &'static str {
        match self {
            Effort::Instant => "instant",
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::Max => "max",
        }
    }

    pub fn parse(s: &str) -> Option<Effort> {
        Effort::ALL.into_iter().find(|e| e.as_str().eq_ignore_ascii_case(s.trim()))
    }
}

/// The level a pack runs at when its reference names none and the pack has
/// no Auto: the middle of the ladder.
pub const DEFAULT_EFFORT: Effort = Effort::Medium;

/// What each level and capability of a pack runs on: `provider/model`
/// strings. An empty level runs on the nearest filled level above it, then
/// below ([`Pack::model_for`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PackLevels {
    /// Janus's own pick per request. Only the built-in Nebo AI pack has it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub low: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub medium: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub high: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<String>,
    /// Describes images when the level's model can't see them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vision: Option<String>,
    /// Calls and spoken replies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
}

impl PackLevels {
    fn level(&self, effort: Effort) -> Option<&str> {
        match effort {
            Effort::Instant => self.instant.as_deref(),
            Effort::Low => self.low.as_deref(),
            Effort::Medium => self.medium.as_deref(),
            Effort::High => self.high.as_deref(),
            Effort::Max => self.max.as_deref(),
        }
        .map(str::trim)
        .filter(|m| !m.is_empty())
    }
}

/// An intelligence pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pack {
    pub id: String,
    pub name: String,
    pub levels: PackLevels,
    /// When this pack's AI can't take a turn (its provider isn't connected),
    /// the turn runs on Nebo AI at the same level and says so. Off: the turn
    /// fails with that line instead.
    pub fallback: bool,
    /// The built-in Nebo AI pack: never edited or deleted.
    pub built_in: bool,
}

/// The built-in pack's id.
pub const NEBO_AI_ID: &str = "nebo-ai";

/// The built-in Nebo AI pack: Janus's speeds (checked live 2026-10-07:
/// `nebo-1-flash` Fast, `nebo-1-medium` Balanced, `nebo-1-pro` Deep) and its
/// Auto alias `nebo-1`, today's default model. Max runs on `nebo-1-pro`
/// until Janus has a real Max.
pub fn nebo_ai() -> Pack {
    let janus = |m: &str| Some(format!("janus/{m}"));
    Pack {
        id: NEBO_AI_ID.to_string(),
        name: "Nebo AI".to_string(),
        levels: PackLevels {
            auto: janus("nebo-1"),
            instant: janus("nebo-1-flash"),
            low: janus("nebo-1-flash"),
            medium: janus("nebo-1-medium"),
            high: janus("nebo-1-pro"),
            max: janus("nebo-1-pro"),
            vision: None,
            voice: None,
        },
        fallback: false,
        built_in: true,
    }
}

impl Pack {
    /// The model `effort` runs on: its own level, else the nearest filled
    /// level above it, else the nearest below. None for a pack with no level
    /// filled.
    pub fn model_for(&self, effort: Effort) -> Option<&str> {
        let above = Effort::ALL.into_iter().filter(|e| *e >= effort);
        let below = Effort::ALL.into_iter().rev().filter(|e| *e < effort);
        above.chain(below).find_map(|e| self.levels.level(e))
    }

    /// The model a reference to this pack runs on: the level it names; with
    /// none, Auto where the pack has it, else [`DEFAULT_EFFORT`].
    pub fn model_for_ref(&self, effort: Option<Effort>) -> Option<&str> {
        match effort {
            Some(e) => self.model_for(e),
            None => self
                .levels
                .auto
                .as_deref()
                .filter(|m| !m.trim().is_empty())
                .or_else(|| self.model_for(DEFAULT_EFFORT)),
        }
    }
}

/// The prefix a model string names a pack with.
pub const REF_PREFIX: &str = "pack/";

/// The pack a model string names, and the level it names (None: the pack's
/// Auto, else its default): `pack/<id>`, `pack/<id>/<level>`, `pack/<id>/auto`.
/// None for any other model string, or an unknown level.
pub fn parse_ref(model: &str) -> Option<(&str, Option<Effort>)> {
    let rest = model.trim().strip_prefix(REF_PREFIX)?;
    let (id, level) = match rest.split_once('/') {
        Some((id, level)) => (id, Some(level)),
        None => (rest, None),
    };
    if id.is_empty() {
        return None;
    }
    match level {
        None => Some((id, None)),
        Some(l) if l.eq_ignore_ascii_case("auto") => Some((id, None)),
        Some(l) => Some((id, Some(Effort::parse(l)?))),
    }
}

/// The model string that names `id` at `effort` (None: Auto / the default).
pub fn pack_ref(id: &str, effort: Option<Effort>) -> String {
    match effort {
        Some(e) => format!("{REF_PREFIX}{id}/{}", e.as_str()),
        None => format!("{REF_PREFIX}{id}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn byo(levels: PackLevels) -> Pack {
        Pack { id: "mine".into(), name: "Mine".into(), levels, fallback: true, built_in: false }
    }

    #[test]
    fn the_ladder_is_ordered_and_round_trips() {
        assert!(Effort::Instant < Effort::Low && Effort::Low < Effort::Medium);
        assert!(Effort::Medium < Effort::High && Effort::High < Effort::Max);
        for e in Effort::ALL {
            assert_eq!(Effort::parse(e.as_str()), Some(e));
            assert_eq!(serde_json::to_string(&e).unwrap(), format!("\"{}\"", e.as_str()));
        }
        assert_eq!(Effort::parse("HIGH"), Some(Effort::High));
        assert_eq!(Effort::parse("deep"), None);
    }

    #[test]
    fn an_empty_level_runs_on_the_nearest_above_then_below() {
        let only_medium = byo(PackLevels { medium: Some("anthropic/sonnet".into()), ..Default::default() });
        for e in Effort::ALL {
            assert_eq!(only_medium.model_for(e), Some("anthropic/sonnet"), "{e:?}");
        }
        let ends = byo(PackLevels {
            low: Some("a/low".into()),
            high: Some("a/high".into()),
            ..Default::default()
        });
        assert_eq!(ends.model_for(Effort::Instant), Some("a/low"), "above first");
        assert_eq!(ends.model_for(Effort::Medium), Some("a/high"), "above first");
        assert_eq!(ends.model_for(Effort::Max), Some("a/high"), "nothing above: below");
        assert_eq!(byo(PackLevels::default()).model_for(Effort::Medium), None);
        let blank = byo(PackLevels { medium: Some("  ".into()), high: Some("a/h".into()), ..Default::default() });
        assert_eq!(blank.model_for(Effort::Medium), Some("a/h"), "a blank level is empty");
    }

    #[test]
    fn only_nebo_ai_has_auto() {
        let nebo = nebo_ai();
        assert_eq!(nebo.model_for_ref(None), Some("janus/nebo-1"), "Auto: Janus picks");
        assert_eq!(nebo.model_for_ref(Some(Effort::Instant)), Some("janus/nebo-1-flash"));
        assert_eq!(nebo.model_for_ref(Some(Effort::Medium)), Some("janus/nebo-1-medium"));
        assert_eq!(nebo.model_for_ref(Some(Effort::Max)), Some("janus/nebo-1-pro"));
        let mine = byo(PackLevels {
            low: Some("a/low".into()),
            medium: Some("a/mid".into()),
            ..Default::default()
        });
        assert_eq!(mine.model_for_ref(None), Some("a/mid"), "no Auto: the default level");
    }

    #[test]
    fn a_pack_reference_parses_and_prints() {
        assert_eq!(parse_ref("pack/nebo-ai"), Some(("nebo-ai", None)));
        assert_eq!(parse_ref(" pack/nebo-ai/auto "), Some(("nebo-ai", None)));
        assert_eq!(parse_ref("pack/x1/high"), Some(("x1", Some(Effort::High))));
        assert_eq!(parse_ref("pack/x1/deep"), None, "an unknown level");
        assert_eq!(parse_ref("pack/"), None);
        assert_eq!(parse_ref("janus/nebo-1"), None);
        assert_eq!(parse_ref("anthropic/claude-sonnet"), None);
        for (id, e) in [("x1", Some(Effort::Low)), ("nebo-ai", None)] {
            assert_eq!(parse_ref(&pack_ref(id, e)), Some((id, e)));
        }
    }

    #[test]
    fn levels_serialize_as_the_api_and_database_carry_them() {
        let p = byo(PackLevels { medium: Some("a/m".into()), vision: Some("a/v".into()), ..Default::default() });
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["builtIn"], false);
        assert_eq!(v["levels"], serde_json::json!({"medium": "a/m", "vision": "a/v"}));
        let back: PackLevels = serde_json::from_value(serde_json::json!({"high": "a/h"})).unwrap();
        assert_eq!(back.high.as_deref(), Some("a/h"));
    }
}
