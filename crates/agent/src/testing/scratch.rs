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

/// A fixture's runs stopped before all `runs` had gone: the traces already
/// collected — including a synthetic trace for the run that failed, carrying
/// the reason — travel with the error so the caller never has to choose
/// between reporting the failure and keeping the evidence. (A judged gate run
/// that threw away a failed fixture's traces once left only the server log as
/// a witness to what went silent.)
pub struct RunBoundError {
    pub message: String,
    pub traces: Vec<Trace>,
}

impl std::fmt::Display for RunBoundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::fmt::Debug for RunBoundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RunBoundError({})", self.message)
    }
}

impl From<RunBoundError> for String {
    fn from(e: RunBoundError) -> String {
        e.message
    }
}

/// Run a fixture once per run number in `runs` (`1..=3` is run-1 to run-3),
/// each run bound to its own scratch directory and its own tag before its
/// setup runs. A caller that gives every run a server of its own asks for one
/// run at a time (`2..=2`), so the run keeps its number. Every caller that runs a fixture goes
/// through here: the engine runs the fixture it is handed, and deciding that
/// there are several runs is what makes a run need a scratch of its own.
///
/// A run that fails ends the fixture here — the remaining runs are never
/// attempted (this is deliberate: a run failure is not retried or skipped
/// past, and changing how many runs execute changes the gate's cost). But the
/// runs already completed, and the one that failed, both leave a trace file:
/// see [`RunBoundError`].
pub async fn run_bound(
    fixture: &Fixture,
    server: &str,
    model: Option<&str>,
    overrides: &HashMap<String, String>,
    runs: std::ops::RangeInclusive<usize>,
) -> Result<Vec<Trace>, RunBoundError> {
    let mut traces = Vec::new();
    for run in runs {
        let run_id = format!("run-{}", run);
        let bound = match prepare(fixture, &run_id) {
            Ok(b) => b,
            Err(e) => {
                let message = format!("run {} scratch setup failed: {}", run_id, e);
                traces.push(Trace::failed(&fixture.id, &run_id, model, &e));
                return Err(RunBoundError { message, traces });
            }
        };
        match engine::run_live(&bound, server, model, overrides, 1).await {
            Ok(run_traces) => {
                for mut trace in run_traces {
                    // The engine numbers the runs it was asked for, and it is
                    // asked for one at a time here; the run this trace
                    // belongs to is ours, and traces are filed on disk
                    // under it.
                    trace.run_id = run_id.clone();
                    traces.push(trace);
                }
            }
            Err(e) => {
                let message = format!("run {} failed: {}", run_id, e);
                traces.push(Trace::failed(&fixture.id, &run_id, model, &e));
                return Err(RunBoundError { message, traces });
            }
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

    /// A run that can't even reach the server (no `make dev` running, or a
    /// port nobody answers) still leaves a trace file for that run, named as
    /// failed and carrying the reason — instead of the fixture's traces
    /// vanishing with only the server log left as a witness. Two runs are
    /// requested; the run that fails ends the fixture (the remaining run is
    /// never attempted), and exactly the one trace comes back.
    #[tokio::test]
    async fn a_failing_run_leaves_a_trace_file_naming_why() {
        let fix = fixture("trace-keep-test");
        let overrides = HashMap::new();

        // Port 1 is reserved for tcpmux and nothing binds it in test
        // environments: the connection is refused immediately rather than
        // timing out, so this stays fast without a real server.
        let err = run_bound(&fix, "127.0.0.1:1", None, &overrides, 1..=2)
            .await
            .expect_err("an unreachable server must fail the run");

        assert_eq!(err.traces.len(), 1, "the failed run's trace only — run 2 is never attempted");
        let trace = &err.traces[0];
        assert_eq!(trace.run_id, "run-1");
        let reason = trace
            .failure_reason
            .as_ref()
            .expect("a failed run's trace must carry why");
        assert!(err.message.contains(reason.as_str()), "{}", err.message);

        let out_dir = tempfile::tempdir().expect("tempdir");
        trace.save(out_dir.path()).expect("save");
        let entries: Vec<_> = std::fs::read_dir(out_dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(entries.len(), 1, "{entries:?}");
        let filename = &entries[0];
        assert!(filename.contains("FAILED"), "{filename}");

        let loaded = Trace::load(&out_dir.path().join(filename)).expect("load saved trace");
        assert_eq!(
            loaded.failure_reason.as_deref(),
            Some(reason.as_str()),
            "the saved trace file must carry the failure reason"
        );
    }

    /// A run asked for on its own keeps its number: the gate gives each
    /// replay run a fresh server and asks for run 2 as `2..=2`, and its trace
    /// is filed as run-2, beside run-1's, not over it.
    #[tokio::test]
    async fn a_run_asked_for_alone_keeps_its_number() {
        let fix = fixture("run-number-test");
        let err = run_bound(&fix, "127.0.0.1:1", None, &HashMap::new(), 2..=2)
            .await
            .expect_err("an unreachable server must fail the run");
        assert_eq!(err.traces.len(), 1);
        assert_eq!(err.traces[0].run_id, "run-2");
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
