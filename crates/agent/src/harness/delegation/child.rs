//! The ONE constructor of a child turn: a first launch, a continuation and a
//! notification turn all use it. The child inherits the
//! parent's seat and can only narrow it (moved from the orchestrator's
//! `build_subagent_request`, #246).

use tokio_util::sync::CancellationToken;

use super::super::{Delivery, SeatRequest, TurnInput, TurnMode, TurnRequest};
use super::{HelperSpec, depth_of, helper_key};
use types::provenance::ProvenanceClass;

/// The turn a helper is started from.
pub struct Parent<'a> {
    pub session_key: &'a str,
    /// The seat the parent's turn asked for. A resolved seat carries no
    /// request (the harness resolves it per turn), so the child is built from
    /// what the parent asked for and resolves its own.
    pub seat: &'a SeatRequest,
    /// The grant the parent's turn holds: the helper's ceiling, and so its
    /// permission mode, rules, money limits and fence. `None` only when the
    /// parent ran without one; the helper then holds its employee's own.
    pub grant: Option<&'a types::permissions::Grant>,
    /// Untrusted content the parent's turn touched before it started the
    /// helper.
    pub run_taint: &'a [ProvenanceClass],
    /// The helper's cancel token, derived from the parent session's stop
    /// token.
    pub cancel: CancellationToken,
}

/// The request for helper `task_id` of `parent`. The helper runs in the
/// parent's permission mode under the parent's grant as its ceiling: it can
/// only narrow. `isolated_copy` is its own copy of the project when it is
/// isolated: its fence narrows to the copy.
pub fn child_request(
    parent: &Parent<'_>,
    task_id: &str,
    spec: &HelperSpec,
    isolated_copy: Option<&str>,
    input: TurnInput,
) -> TurnRequest {
    let session_key = helper_key(parent.session_key, task_id);
    let p = parent.seat;

    let mut seed_taint = p.seed_taint.clone();
    for class in parent.run_taint {
        if !seed_taint.contains(class) {
            seed_taint.push(*class);
        }
    }
    let ceiling = parent.grant.map(|g| {
        let mut grant = g.clone();
        if let Some(copy) = isolated_copy {
            grant.fence = Some(vec![std::path::PathBuf::from(copy)]);
        }
        types::permissions::Ceiling::Parent { grant: Box::new(grant) }
    });
    let cwd = isolated_copy.map(str::to_string).or_else(|| p.cwd.clone());

    let seat = SeatRequest {
        agent_id: p.agent_id.clone(),
        user_id: p.user_id.clone(),
        // A helper is never the owner: an owner's run starts its helpers as
        // the system. Every other origin (an outside caller, a coworker, an
        // MCP client) stays what it was, with its limits.
        origin: match p.origin {
            tools::Origin::User => tools::Origin::System,
            other => other,
        },
        door: types::permissions::Door::Helper,
        // The parent's mode, including a run override such as Plan; the
        // ceiling carries the rest of its grant.
        mode: parent.grant.map(|g| g.mode).or(p.mode),
        ceiling,
        cwd,
        seed_taint,
        audience: p.audience.clone(),
        tool_allowlist: p.tool_allowlist.clone(),
        tool_denial_hint: p.tool_denial_hint.clone(),
        // A helper stays at its parent's hop depth: a helper must not
        // restart the coworker chain cap at zero.
        handoff_depth: p.handoff_depth,
        // The speed the helper was given, else its parent's model.
        model_override: spec.speed.clone().unwrap_or_else(|| p.model_override.clone()),
        model_preference: p.model_preference.clone(),
        personality_snippet: None,
        tool_scope: p.tool_scope.clone(),
    };

    TurnRequest {
        mode: TurnMode::Helper {
            parent_session_key: parent.session_key.to_string(),
            kind: spec.kind,
            depth: depth_of(&session_key),
            answer: None,
        },
        session_key,
        input,
        seat,
        delivery: Delivery { channel: "subagent".to_string(), channel_ctx: None, mention_briefing: None },
        cancel: parent.cancel.clone(),
        progress: None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::HelperKind;
    use super::*;
    use types::permissions::{Ceiling, Door, Grant, Mode};

    pub(crate) fn parent_seat() -> SeatRequest {
        SeatRequest {
            agent_id: "bookkeeper".into(),
            user_id: "owner-1".into(),
            origin: tools::Origin::User,
            door: Door::Chat,
            mode: None,
            ceiling: None,
            cwd: Some("/work/books".into()),
            seed_taint: vec![ProvenanceClass::Web],
            audience: None,
            tool_allowlist: None,
            tool_denial_hint: None,
            handoff_depth: 1,
            model_override: String::new(),
            model_preference: Some("fast".into()),
            personality_snippet: Some("warm".into()),
            tool_scope: None,
        }
    }

    pub(crate) fn parent_grant(mode: Mode) -> Grant {
        Grant {
            agent_id: "bookkeeper".into(),
            mode,
            rules: vec![],
            ceiling: None,
            run_folders: vec!["/work/books".into()],
            fence: Some(vec!["/work/books".into()]),
        }
    }

    fn spec(kind: HelperKind) -> HelperSpec {
        HelperSpec {
            description: "read the ledger".into(),
            prompt: "Find the missing invoice.".into(),
            kind,
            background: true,
            isolation: None,
            skills: Vec::new(),
            speed: None,
        }
    }

    fn ceiling_of(req: &TurnRequest) -> &Grant {
        match req.seat.ceiling.as_ref().expect("a helper runs under its parent's grant") {
            Ceiling::Parent { grant } => grant,
            other => panic!("not a parent ceiling: {other:?}"),
        }
    }

    #[test]
    fn a_child_inherits_the_parents_limits_and_narrows_only() {
        let seat = parent_seat();
        let grant = parent_grant(Mode::Automatic);
        let parent = Parent {
            session_key: "agent:bookkeeper:web",
            seat: &seat,
            grant: Some(&grant),
            run_taint: &[ProvenanceClass::Phone],
            cancel: CancellationToken::new(),
        };
        let req = child_request(&parent, "h-1", &spec(HelperKind::General), None, TurnInput::None);
        assert_eq!(req.session_key, "subagent:agent:bookkeeper:web:h-1");
        assert_eq!(ceiling_of(&req), &grant, "the parent's whole grant is the ceiling");
        assert_eq!(req.seat.door, Door::Helper);
        assert_eq!(req.seat.agent_id, "bookkeeper");
        assert_eq!(req.seat.user_id, "owner-1");
        assert_eq!(req.seat.origin, tools::Origin::System, "a helper is never the owner");
        assert_eq!(req.seat.seed_taint, vec![ProvenanceClass::Web, ProvenanceClass::Phone]);
        assert_eq!(req.seat.handoff_depth, 1);
        assert_eq!(req.seat.model_preference.as_deref(), Some("fast"));
        assert!(req.seat.personality_snippet.is_none());
        assert!(matches!(req.mode, TurnMode::Helper { depth: 1, kind: HelperKind::General, .. }));

        let isolated = child_request(&parent, "h-2", &spec(HelperKind::General), Some("/tmp/copy"), TurnInput::None);
        assert_eq!(ceiling_of(&isolated).fence, Some(vec!["/tmp/copy".into()]), "the fence narrows to the copy");
        assert_eq!(isolated.seat.cwd.as_deref(), Some("/tmp/copy"));

        let outside = SeatRequest { origin: tools::Origin::Caller, ..parent_seat() };
        let parent = Parent { seat: &outside, ..parent };
        let req = child_request(&parent, "h-3", &spec(HelperKind::General), None, TurnInput::None);
        assert_eq!(req.seat.origin, tools::Origin::Caller, "an outside origin keeps its limits");
    }

    /// A helper runs in its parent's permission mode: a Plan parent's helper
    /// only plans, a Full Access parent's helper doesn't ask.
    #[test]
    fn a_child_inherits_the_parents_permission_mode() {
        let seat = parent_seat();
        for mode in [Mode::Automatic, Mode::Ask, Mode::Plan, Mode::FullAccess] {
            let grant = parent_grant(mode);
            let parent = Parent {
                session_key: "agent:bookkeeper:web",
                seat: &seat,
                grant: Some(&grant),
                run_taint: &[],
                cancel: CancellationToken::new(),
            };
            let req = child_request(&parent, "h-1", &spec(HelperKind::General), None, TurnInput::None);
            assert_eq!(req.seat.mode, Some(mode));
            assert_eq!(ceiling_of(&req).mode, mode);
        }
        // A parent whose run overrides its mode (a Plan run) passes that on.
        let planning = SeatRequest { mode: Some(Mode::Plan), ..parent_seat() };
        let parent = Parent {
            session_key: "agent:bookkeeper:web",
            seat: &planning,
            grant: None,
            run_taint: &[],
            cancel: CancellationToken::new(),
        };
        let req = child_request(&parent, "h-1", &spec(HelperKind::General), None, TurnInput::None);
        assert_eq!(req.seat.mode, Some(Mode::Plan));
    }

    /// A helper works at its parent's speed unless it was given its own
    /// (fix plan E8): the speed is the model its every step runs on.
    #[test]
    fn a_helper_works_at_its_own_speed_when_given_one() {
        let seat = SeatRequest { model_override: "janus/nebo-1".into(), ..parent_seat() };
        let parent = Parent {
            session_key: "agent:bookkeeper:web",
            seat: &seat,
            grant: None,
            run_taint: &[],
            cancel: CancellationToken::new(),
        };
        let inherits = child_request(&parent, "h-1", &spec(HelperKind::Explore), None, TurnInput::None);
        assert_eq!(inherits.seat.model_override, "janus/nebo-1", "the parent's speed");
        let fast = HelperSpec { speed: Some("janus/nebo-1-fast".into()), ..spec(HelperKind::Explore) };
        let own = child_request(&parent, "h-2", &fast, None, TurnInput::None);
        assert_eq!(own.seat.model_override, "janus/nebo-1-fast", "its own speed");
    }
}
