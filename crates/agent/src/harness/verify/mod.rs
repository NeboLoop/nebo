//! Verified: code work is not reported done until independent reviewers in
//! fresh contexts fail to break it and the project's own build and tests
//! pass.
//!
//! It applies only to a turn that had code tools ([`code_surface`]) and
//! changed a project's files (a [`shadow::ChangeSet`] that is not empty).
//! Every other turn runs no check and costs nothing more.
//!
//! The parts, each its own module:
//! - [`shadow`]: what changed, the whole change and each round's delta;
//! - [`project`]: which folder a change belongs to;
//! - [`lenses`]: the review lenses a change gets, and whether it is small;
//! - [`checks`]: the project's own build and tests;
//! - [`rules`]: the rules every brief carries;
//! - [`findings`]: what a reviewer reports, and when the loop has stalled.

pub mod checks;
pub mod findings;
pub mod lenses;
pub mod project;
pub mod rules;
pub mod shadow;

/// The deferred tools that make a turn's surface a code surface: the code
/// intelligence tool, an isolated machine, notebooks, file checkpoints, and
/// the app developer pack. The always-loaded file and shell tools are every
/// employee's and say nothing about code on their own.
pub const CODE_TOOLS: &[&str] = &[
    "code",
    "vm",
    "notebook",
    "checkpoint_files",
    "restore_checkpoint",
];

/// Whether a turn works on code: one of [`CODE_TOOLS`] or the app developer
/// pack is declared to it, it is a linked coding agent's turn (`linked`),
/// or it is a helper working in its own copy of a project (`isolated`).
pub fn code_surface<'a>(
    declared: impl IntoIterator<Item = &'a str>,
    linked: bool,
    isolated: bool,
) -> bool {
    linked
        || isolated
        || declared
            .into_iter()
            .any(|name| CODE_TOOLS.contains(&name) || tools::app_dev::TOOLS.contains(&name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_code_tools_a_coding_agent_or_an_isolated_copy_make_a_code_surface() {
        let everyone = [
            "read_file",
            "write_file",
            "edit_file",
            "run_command",
            "delegate",
        ];
        assert!(
            !code_surface(everyone, false, false),
            "the always-loaded tools are every employee's"
        );
        assert!(code_surface(
            everyone.into_iter().chain(["code"]),
            false,
            false
        ));
        assert!(code_surface(
            everyone.into_iter().chain(["app_reload"]),
            false,
            false
        ));
        assert!(code_surface(everyone, true, false));
        assert!(code_surface(everyone, false, true));
    }
}
