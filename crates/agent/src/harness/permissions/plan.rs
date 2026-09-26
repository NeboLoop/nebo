//! Plan mode: read and plan only. A call that changes something doesn't
//! run; the plan document does, since writing the plan is the point of the
//! mode. `exit_plan_mode` is the way out: it always asks the owner, on the
//! one ask card ("Exit plan mode?"), and approving it switches the employee
//! out of Plan mode.

use types::permissions::{AskCase, Decision, Mode, Target, Why};

/// What a change hears in Plan mode.
pub const REFUSAL: &str =
    "Plan mode: this changes something, so it didn't run. Include this step in the plan instead.";

/// What `exit_plan_mode` hears outside Plan mode: there is nothing to
/// leave, and an approved plan is carried out.
pub const NOT_IN_PLAN_MODE: &str = "You are not in plan mode. This tool only leaves plan mode once a plan is \
written. If your plan was already approved, carry it out.";

/// The tool that writes the plan document.
const PLAN_DOCUMENT: &str = "write_plan";

/// Whether Plan mode lets `t` run: a read, or the plan document.
pub fn allows(t: &Target) -> bool {
    t.read_only || t.key == PLAN_DOCUMENT
}

/// The way out of Plan mode: refused outside it, and in it always the
/// owner's to answer. `None` for every other call.
pub fn exit(mode: Mode, t: &Target) -> Option<Decision> {
    if t.key != tools::file_tools::ExitPlanModeTool::NAME {
        return None;
    }
    Some(match mode {
        Mode::Plan => Decision::Ask { case: AskCase::Widens },
        mode => Decision::Deny { reason: NOT_IN_PLAN_MODE.to_string(), why: Why::Mode { mode } },
    })
}
