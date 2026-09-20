//! Per-run binding of a fixture to its own scratch space.
//!
//! Fixtures used to name fixed paths (`/tmp/nebo-test/notes.txt`) and fixed
//! employees (`front-desk`). Four fixtures shared one directory and three of
//! them `rm -rf`'d it in teardown; two created and deleted the same employee.
//! Run one at a time that is merely untidy; run side by side it is fixtures
//! deleting each other's work, which is why the judged gate could never be
//! pooled.
//!
//! So fixture text carries two variables, and every fixture run is bound to
//! its own values before setup runs:
//!
//! - `{{scratch}}` — a directory of this fixture's own, for this run, under
//!   [`ROOT`]. [`run_bound`] creates it before setup runs and the fixture's
//!   teardown removes it.
//! - `{{tag}}` — a short token unique to the fixture and the run, for names
//!   that live on the server rather than on disk: an employee a fixture hires
//!   is `front-desk-{{tag}}`, and teardown deletes exactly that one.
//!
//! Both are derived from the fixture id and the run id, so the same run always
//! renders the same text and the same run can be found again.

use std::collections::HashMap;

use sha2::{Digest, Sha256};

use super::engine;
use super::fixture::Fixture;
use super::trace::Trace;

/// The one directory the harness owns. Every fixture's scratch lives under it,
/// so a stopped run leaves its litter in a single place.
pub const ROOT: &str = "/tmp/nebo-eval";

/// This fixture-run's own directory. Nothing else writes here.
///
/// It is named by the [`tag`], not by the fixture: the model reads these paths,
/// and `/tmp/nebo-eval/correction-stop-means-stop/part1.txt` would tell it what
/// the fixture is watching for. A token says nothing and is still this run's
/// alone.
pub fn dir(fixture_id: &str, run_id: &str) -> String {
    format!("{ROOT}/{}", tag(fixture_id, run_id))
}

/// A short token unique to this fixture and run: the name of its scratch
/// directory, and the suffix on every employee it hires.
pub fn tag(fixture_id: &str, run_id: &str) -> String {
    let digest = Sha256::digest(format!("{fixture_id}/{run_id}").as_bytes());
    digest[..4].iter().map(|b| format!("{b:02x}")).collect()
}

/// Render `{{scratch}}` and `{{tag}}` through the whole fixture — setup,
/// teardown, conversation, cwd and assertions alike — by round-tripping it
/// through YAML, so no field can be forgotten.
pub fn bind(fixture: &Fixture, run_id: &str) -> Result<Fixture, String> {
    Ok(rendered(fixture, run_id)?.0)
}

/// Bind the fixture to this run and, when it asks for one, create its scratch
/// directory — a fixture whose files are written by the model rather than by
/// setup still needs somewhere of its own to write.
pub fn prepare(fixture: &Fixture, run_id: &str) -> Result<Fixture, String> {
    let (bound, wants_dir) = rendered(fixture, run_id)?;
    if wants_dir {
        let dir = dir(&fixture.id, run_id);
        std::fs::create_dir_all(&dir).map_err(|e| format!("create scratch {dir}: {e}"))?;
    }
    Ok(bound)
}

/// The bound fixture, and whether it asked for a scratch directory at all.
fn rendered(fixture: &Fixture, run_id: &str) -> Result<(Fixture, bool), String> {
    let text = serde_yaml::to_string(fixture)
        .map_err(|e| format!("render fixture {}: {}", fixture.id, e))?;
    let wants_dir = text.contains(SCRATCH);
    let bound = serde_yaml::from_str(&render(&text, &fixture.id, run_id))
        .map_err(|e| format!("re-read fixture {}: {}", fixture.id, e))?;
    Ok((bound, wants_dir))
}

/// Run a fixture `runs` times, each run bound to its own scratch directory and
/// its own tag before its setup runs. Every caller that runs a fixture goes
/// through here: the engine runs the fixture it is handed, and deciding that
/// there are several runs is what makes a run need a scratch of its own.
pub async fn run_bound(
    fixture: &Fixture,
    server: &str,
    model: Option<&str>,
    overrides: &HashMap<String, String>,
    runs: usize,
) -> Result<Vec<Trace>, String> {
    let mut traces = Vec::new();
    for run_idx in 0..runs {
        let run_id = format!("run-{}", run_idx + 1);
        let bound = prepare(fixture, &run_id)?;
        for mut trace in engine::run_live(&bound, server, model, overrides, 1).await? {
            // The engine numbers the runs it was asked for, and it is asked
            // for one at a time here; the run this trace belongs to is ours,
            // and traces are filed on disk under it.
            trace.run_id = run_id.clone();
            traces.push(trace);
        }
    }
    Ok(traces)
}

const SCRATCH: &str = "{{scratch}}";
const TAG: &str = "{{tag}}";

fn render(text: &str, fixture_id: &str, run_id: &str) -> String {
    text.replace(SCRATCH, &dir(fixture_id, run_id))
        .replace(TAG, &tag(fixture_id, run_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::fixture::Fixture;

    fn fixture(id: &str) -> Fixture {
        serde_yaml::from_str(&format!(
            r#"
id: {id}
name: t
setup:
  - 'printf x > {{{{scratch}}}}/f.txt'
teardown:
  - 'rm -rf {{{{scratch}}}}'
conversation:
  - role: user
    content: 'read {{{{scratch}}}}/f.txt as front-desk-{{{{tag}}}}'
"#
        ))
        .expect("test fixture")
    }

    #[test]
    fn every_field_is_rendered_and_two_runs_never_share() {
        let one = bind(&fixture("a"), "run-1").unwrap();
        let two = bind(&fixture("a"), "run-2").unwrap();
        for f in [&one, &two] {
            assert!(!format!("{f:?}").contains("{{"), "a variable survived: {f:?}");
        }
        assert_ne!(one.setup, two.setup);
        assert_ne!(one.conversation[0].content, two.conversation[0].content);
        assert!(one.teardown[0].contains(&dir("a", "run-1")));
    }

    #[test]
    fn two_fixtures_never_share_a_directory_or_a_tag() {
        assert_ne!(dir("a", "run-1"), dir("b", "run-1"));
        assert_ne!(tag("a", "run-1"), tag("b", "run-1"));
        assert_ne!(tag("a", "run-1"), tag("a", "run-2"));
        assert!(dir("a", "run-1").starts_with(ROOT));
    }

    #[test]
    fn a_fixture_with_no_variables_is_unchanged() {
        let mut plain = fixture("a");
        plain.setup = vec!["true".into()];
        plain.teardown = Vec::new();
        plain.conversation[0].content = "hello".into();
        let bound = bind(&plain, "run-1").unwrap();
        assert_eq!(bound.setup, plain.setup);
        assert_eq!(bound.conversation[0].content, "hello");
    }
}
