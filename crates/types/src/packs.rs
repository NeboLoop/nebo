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
/// strings. An empty level of an owner's pack runs on Nebo AI's model for
/// that same level (owner, 2026-10-11: "if it's not set it defaults to the
/// Nebo one for that level"); [`Pack::model_for`] says which levels are set.
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
    /// Reads the turn's images and video (the vision helper). Empty: the
    /// level's model when it sees images, else Nebo AI's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vision: Option<String>,
    /// Calls and spoken replies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// Quick yes/no and pick-one judgments (permissions, triage, memory):
    /// a decision model (Jev or a SystemOne-compatible one). Empty: Nebo
    /// AI's Jev through Janus.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decisions: Option<String>,
}

/// Kinds of work pinned to an Effort level. Absent: the work runs at the
/// level the owner chose, except background work (heartbeat, schedules, a
/// workflow's steps), which runs at Instant unless the pack pins it
/// ([`Lane::is_background`]), and building with no level chosen, which runs
/// at [`DEFAULT_EFFORT`] ([`PackLanes::level`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PackLanes {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heartbeat: Option<Effort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled: Option<Effort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub communication: Option<Effort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub helpers: Option<Effort>,
    /// Designing employees and workflows: the steps that write them. Absent:
    /// the level the owner chose in the composer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<Effort>,
    /// A workflow's steps as they run, unless the step declares its own
    /// level (the activity's `level`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow: Option<Effort>,
}

/// Work a pack can pin to a level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    Heartbeat,
    Scheduled,
    Communication,
    Helpers,
    /// Writing an employee or a workflow (`create_employee`,
    /// `create_workflow`, the workflow builder). Runs at the level the owner
    /// chose unless the pack pins it; with no level chosen (Auto, or a
    /// session that builds before anyone chose), at [`DEFAULT_EFFORT`],
    /// never dropped to Instant.
    Build,
    /// Running a workflow's steps.
    Workflow,
    /// Background work for the owner's turns and employees: titles, memory,
    /// tool summaries, permission and goal checks. Always the pack's
    /// Instant level.
    Housekeeping,
    /// An Explore helper's wide read-only search: many quick reads whose
    /// findings the helper that asked weighs. Always the pack's Instant
    /// level, as Claude Code runs its Explore agent on its fast model.
    Explore,
}

impl Lane {
    /// Work that runs with nobody waiting on its reply: a heartbeat, a
    /// schedule, a workflow's step. It runs at the level its pack pins it
    /// to, Instant when the pack pins none, whatever model the employee or
    /// the bot names (owner, 2026-10-16: workflow steps on a chat model cost
    /// a hundred times what they cost on Instant).
    pub fn is_background(self) -> bool {
        matches!(self, Lane::Heartbeat | Lane::Scheduled | Lane::Workflow)
    }
}

impl PackLanes {
    /// The level work in `lane` runs at when the owner chose `chosen` (None:
    /// Auto, no level). A level the pack pins wins. Background work
    /// ([`Lane::is_background`]) is always pinned: Instant unless the pack
    /// pins it elsewhere. Building keeps the level the owner chose, and with
    /// none runs at [`DEFAULT_EFFORT`] (owner, 2026-10-18: "you choose" is
    /// honest; we publish which level to pick to design a workflow instead
    /// of forcing one). The rest runs at the level chosen.
    pub fn level(&self, lane: Lane, chosen: Option<Effort>) -> Option<Effort> {
        match lane {
            Lane::Heartbeat => self.heartbeat.or(Some(Effort::Instant)),
            Lane::Scheduled => self.scheduled.or(Some(Effort::Instant)),
            Lane::Communication => self.communication.or(chosen),
            Lane::Helpers => self.helpers.or(chosen),
            Lane::Build => self.build.or(chosen).or(Some(DEFAULT_EFFORT)),
            Lane::Workflow => self.workflow.or(Some(Effort::Instant)),
            Lane::Housekeeping | Lane::Explore => Some(Effort::Instant),
        }
    }
}

/// A provider's own reasoning effort, set per level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderEffort {
    Low,
    Medium,
    High,
}

/// The provider effort each level runs at. Absent: the provider's default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LevelEffort {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instant: Option<ProviderEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub low: Option<ProviderEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub medium: Option<ProviderEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub high: Option<ProviderEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<ProviderEffort>,
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
    #[serde(default)]
    pub lanes: PackLanes,
    #[serde(default)]
    pub level_effort: LevelEffort,
}

/// The built-in pack's id.
pub const NEBO_AI_ID: &str = "nebo-ai";

/// The built-in Nebo AI pack: Auto on Janus's `nebo-1` alias, today's
/// default model, and the five levels on Janus's three speeds: Instant and
/// Low on `nebo-1-flash`, Medium on `nebo-1-medium`, High and Max on
/// `nebo-1-pro`. Max runs on `nebo-1-pro` until Janus has a real Max.
///
/// Its lanes: workflows run at Instant. Building is not pinned: employees
/// and workflows are written at the level the owner chose in the composer,
/// at Medium when he chose none (Auto), never at Instant (owner,
/// 2026-10-18, replacing the 2026-10-15 Medium pin: "you choose" is honest,
/// and we publish best practice, High or Max to design a workflow that then
/// runs at Instant, instead of forcing a level). A build's steps reach
/// Janus as the `build` purpose with the chosen level's model alias and
/// effort, which Janus honors.
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
            // Janus reads images and video on its image_read and
            // video_read pools.
            vision: janus("nebo-1"),
            voice: Some(NEBO_AI_VOICE.to_string()),
            decisions: janus("jev-latest"),
        },
        fallback: false,
        built_in: true,
        lanes: PackLanes { workflow: Some(Effort::Instant), ..PackLanes::default() },
        // Low is Instant with a little thinking; Max is High at its highest
        // effort until Janus has a speed above `nebo-1-pro`.
        level_effort: LevelEffort {
            low: Some(ProviderEffort::Low),
            max: Some(ProviderEffort::High),
            ..LevelEffort::default()
        },
    }
}

impl Pack {
    /// The model this pack sets for `effort`: its own level only. None for
    /// an empty level, which runs on Nebo AI's model for the same level
    /// (the turn resolves that, `agent::harness::turn`).
    pub fn model_for(&self, effort: Effort) -> Option<&str> {
        self.levels.level(effort)
    }

    /// The provider effort `effort` runs at, when the pack sets one.
    pub fn effort_for(&self, effort: Effort) -> Option<ProviderEffort> {
        let e = &self.level_effort;
        match effort {
            Effort::Instant => e.instant,
            Effort::Low => e.low,
            Effort::Medium => e.medium,
            Effort::High => e.high,
            Effort::Max => e.max,
        }
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

/// Nebo AI's Voice: Janus's realtime relay, asked for its voice speed. The
/// speed serves only the realtime protocol calls speak today (xAI's); a
/// second protocol joins it only once calls can speak it.
pub const NEBO_AI_VOICE: &str = "janus/nebo-1-voice";

/// The realtime model an xAI connection is asked for when it lists none of
/// its own.
pub const XAI_VOICE_MODEL: &str = "grok-voice-latest";

/// Whether a connection (its provider kind) is xAI's, the only realtime
/// protocol calls speak today: the built-in xAI kind, whose key goes through
/// Janus. A URL the owner typed is his own endpoint, reached direct, never
/// read as xAI's.
pub fn is_xai_connection(kind: &str) -> bool {
    kind.split('@').next() == Some("xai")
}

/// Whether `model` (a model name, no provider) is one of xAI's realtime
/// voice models (`grok-voice-latest`).
pub fn is_xai_voice_model(model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    m.starts_with("grok") && m.contains("voice")
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
        Pack { id: "mine".into(), name: "Mine".into(), levels, fallback: true, built_in: false, level_effort: LevelEffort::default(), lanes: PackLanes::default() }
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
    fn an_empty_level_is_left_to_nebo_ai_never_borrowed_from_another_level() {
        let only_medium = byo(PackLevels { medium: Some("anthropic/sonnet".into()), ..Default::default() });
        assert_eq!(only_medium.model_for(Effort::Medium), Some("anthropic/sonnet"));
        for e in [Effort::Instant, Effort::Low, Effort::High, Effort::Max] {
            assert_eq!(only_medium.model_for(e), None, "{e:?} is Nebo AI's, not Medium's");
        }
        assert_eq!(byo(PackLevels::default()).model_for(Effort::Medium), None);
        let blank = byo(PackLevels { medium: Some("  ".into()), high: Some("a/h".into()), ..Default::default() });
        assert_eq!(blank.model_for(Effort::Medium), None, "a blank level is empty");
        let only_high = byo(PackLevels { high: Some("a/h".into()), ..Default::default() });
        assert_eq!(only_high.model_for_ref(None), None, "no Auto and no Medium: Nebo AI's Medium");
    }

    #[test]
    fn only_nebo_ai_has_auto() {
        let nebo = nebo_ai();
        assert_eq!(nebo.model_for_ref(None), Some("janus/nebo-1"), "Auto: Janus picks");
        assert_eq!(nebo.model_for_ref(Some(Effort::Instant)), Some("janus/nebo-1-flash"));
        assert_eq!(nebo.model_for_ref(Some(Effort::Medium)), Some("janus/nebo-1-medium"));
        assert_eq!(nebo.model_for_ref(Some(Effort::Max)), Some("janus/nebo-1-pro"));
        assert_eq!(nebo.effort_for(Effort::Low), Some(ProviderEffort::Low), "Instant, thinking a little");
        assert_eq!(nebo.effort_for(Effort::High), None, "nebo-1-pro at its default");
        assert_eq!(nebo.effort_for(Effort::Max), Some(ProviderEffort::High), "nebo-1-pro at its highest");
        assert_eq!(nebo.lanes.build, None, "building is not pinned");
        assert_eq!(nebo.lanes.level(Lane::Build, Some(Effort::High)), Some(Effort::High), "built at the level chosen");
        assert_eq!(nebo.lanes.level(Lane::Build, Some(Effort::Instant)), Some(Effort::Instant), "even Instant, when chosen");
        assert_eq!(nebo.lanes.level(Lane::Build, None), Some(Effort::Medium), "on Auto, at Medium");
        assert_eq!(nebo.model_for(Effort::Medium), Some("janus/nebo-1-medium"));
        assert_eq!(nebo.lanes.level(Lane::Workflow, Some(Effort::Max)), Some(Effort::Instant), "run at Instant");
        assert_eq!(nebo.model_for(Effort::Instant), Some("janus/nebo-1-flash"));
        assert_eq!(nebo.lanes.level(Lane::Heartbeat, None), Some(Effort::Instant), "background work at Instant");
        assert_eq!(nebo.lanes.level(Lane::Scheduled, Some(Effort::High)), Some(Effort::Instant), "background work at Instant");
        assert_eq!(nebo.lanes.level(Lane::Communication, None), None, "the rest: the level chosen");
        assert_eq!(nebo.lanes.level(Lane::Communication, Some(Effort::Low)), Some(Effort::Low), "the rest: the level chosen");
        let pinned = PackLanes { heartbeat: Some(Effort::Low), build: Some(Effort::Max), ..PackLanes::default() };
        assert_eq!(pinned.level(Lane::Heartbeat, None), Some(Effort::Low), "a pack's own pin wins");
        assert_eq!(pinned.level(Lane::Build, Some(Effort::Low)), Some(Effort::Max), "a pack that pins Building moves it");
        assert_eq!(PackLanes::default().level(Lane::Workflow, None), Some(Effort::Instant), "an owner's pack too");
        assert_eq!(nebo.lanes.level(Lane::Explore, Some(Effort::Max)), Some(Effort::Instant), "an Explore helper at Instant");
        assert_eq!(PackLanes { helpers: Some(Effort::Max), ..PackLanes::default() }.level(Lane::Explore, None), Some(Effort::Instant));
        let mine = byo(PackLevels {
            low: Some("a/low".into()),
            medium: Some("a/mid".into()),
            ..Default::default()
        });
        assert_eq!(mine.model_for_ref(None), Some("a/mid"), "no Auto: the default level");
    }

    #[test]
    fn nebo_ai_speaks_through_the_janus_voice_relay_and_only_xai_voice_models_are_realtime() {
        assert_eq!(nebo_ai().levels.voice.as_deref(), Some("janus/nebo-1-voice"));
        assert!(is_xai_voice_model("grok-voice-latest") && is_xai_voice_model(XAI_VOICE_MODEL));
        assert!(!is_xai_voice_model("grok-4") && !is_xai_voice_model("gpt-realtime-2") && !is_xai_voice_model("nebo-1-voice"));
        assert!(is_xai_connection("xai") && is_xai_connection("xai@k1"));
        assert!(!is_xai_connection("openai_compatible@k1") && !is_xai_connection("openai@k1") && !is_xai_connection("janus"));
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

    #[test]
    fn lanes_and_effort_carry_as_the_contract_names_them() {
        let mut p = byo(PackLevels { decisions: Some("systemone_compatible@k1/jev-latest".into()), ..Default::default() });
        p.lanes.heartbeat = Some(Effort::Instant);
        p.level_effort.max = Some(ProviderEffort::High);
        let v = serde_json::to_value(&p).unwrap();
        assert!(v.get("routeThroughJanus").is_none(), "no pack routes through Janus by choice: {v}");
        assert_eq!(v["lanes"], serde_json::json!({"heartbeat": "instant"}));
        assert_eq!(v["levelEffort"], serde_json::json!({"max": "high"}));
        assert_eq!(v["levels"]["decisions"], "systemone_compatible@k1/jev-latest");
        // A pack saved before these fields reads with their defaults.
        let old: Pack = serde_json::from_value(serde_json::json!({"id": "o", "name": "Old", "levels": {}, "fallback": true, "builtIn": false})).unwrap();
        assert!(old.lanes == PackLanes::default());
        // One saved with the old toggle reads the same: the toggle is gone.
        let toggled: Pack = serde_json::from_value(serde_json::json!({"id": "t", "name": "T", "levels": {}, "fallback": true, "builtIn": false, "routeThroughJanus": true})).unwrap();
        assert_eq!(toggled.lanes, PackLanes::default());
        let lanes: PackLanes = serde_json::from_value(serde_json::json!({"build": "max", "workflow": "instant"})).unwrap();
        assert_eq!((lanes.level(Lane::Build, None), lanes.level(Lane::Workflow, None)), (Some(Effort::Max), Some(Effort::Instant)));
    }
}
