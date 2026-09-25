//! Fire-time pre-flight: before a workflow binding's timer fire starts (and
//! before heartbeat triage is asked), check in code that what the binding
//! declares it needs is present right now. No model is asked and no token is
//! spent: a binding whose needs are missing does not fire, its record says
//! which need (`agent_workflows.degraded_reason`), and it stays armed. Every
//! fire checks again, so the fire after the need appears runs with no other
//! action.
//!
//! A binding's declared needs, and the ONE place each is resolved:
//! - its watch trigger's capability (a capability name such as `mail`, not a
//!   plugin slug): an installed plugin must bind it —
//!   [`agent::agent_worker::resolve_capability_plugin`], the resolution the
//!   watch trigger itself uses at start, with its reason
//!   ([`agent::agent_worker::capability_degraded_reason`]);
//! - the employee's `requires.plugins`: each plugin installed and not
//!   switched off, whether named by slug, qualified name or install code
//!   ([`required_plugin`], the reading the job's tools use).
//!
//! Not checked: the employee's `requires.interfaces`. It lists every
//! capability the seat may use (its approvals vocabulary), not what one
//! binding needs, so it cannot hold a single binding.
//!
//! A binding that declares nothing passes.

use tracing::{debug, info, warn};

use db::EngineRun;
use db::Store;

/// The plugin a `requires.plugins` entry names, as the need names it: the
/// installed plugin's slug ([`tools::plugin_tools::plugin_slug_of`], the one
/// reading of a job's plugin reference), or for an install code nothing here
/// was installed from, the code, which no installed plugin matches.
pub(crate) fn required_plugin(store: &Store, reference: &str) -> String {
    tools::plugin_tools::plugin_slug_of(store, reference).unwrap_or_else(|| reference.trim().to_string())
}

/// What the binding declares it needs that is not present now, as its
/// record should say it, or None when every need is present. `installed` is
/// every installed plugin with the interfaces it binds
/// ([`agent::agent_worker::installed_interfaces`]); `enabled` the installed
/// plugins that are not switched off.
pub(crate) fn unmet_need(
    store: &Store,
    config: &napp::agent::AgentConfig,
    binding: &napp::agent::WorkflowBinding,
    installed: &[(String, Vec<String>)],
    enabled: &[String],
) -> Option<String> {
    if let napp::agent::AgentTrigger::Watch { plugin, .. } = &binding.trigger {
        let is_slug = installed.iter().any(|(slug, _)| slug == plugin) || plugin.contains('.');
        if !is_slug
            && agent::agent_worker::resolve_capability_plugin(plugin, installed, &[]).is_none()
        {
            return Some(agent::agent_worker::capability_degraded_reason(
                plugin, installed,
            ));
        }
    }
    for reference in &config.requires.plugins {
        let name = required_plugin(store, reference);
        if !installed.iter().any(|(slug, _)| *slug == name) {
            return Some(match crate::codes::detect_code(&name) {
                Some(_) => format!("needs the plugin from code {name}"),
                None => format!("needs the {name} plugin"),
            });
        }
        if !enabled.iter().any(|slug| *slug == name) {
            return Some(format!("needs the {name} plugin turned on"));
        }
    }
    None
}

/// The employee and binding a queued fire runs: a binding heartbeat
/// (`command: agent:<id>:<binding>`) or a schedule of a workflow binding.
/// None for every other fire (an employee's prompt job, a shell job, an
/// entity heartbeat): those declare nothing.
pub(crate) fn fire_binding(store: &Store, run: &EngineRun) -> Option<(String, String)> {
    let inputs: serde_json::Value = run
        .inputs
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let command = match inputs["job_id"].as_i64() {
        Some(id) => {
            let job = store.get_cron_job(id).ok().flatten()?;
            matches!(job.task_type.as_str(), "agent_workflow" | "role_workflow")
                .then_some(job.command)?
        }
        None => inputs["command"].as_str()?.to_string(),
    };
    match command.splitn(3, ':').collect::<Vec<_>>()[..] {
        ["agent" | "role", agent_id, binding] if !agent_id.is_empty() && !binding.is_empty() => {
            Some((agent_id.to_string(), binding.to_string()))
        }
        _ => None,
    }
}

/// The unmet need of one binding right now, read from its employee's
/// definition and the installed plugins. An employee or binding that cannot
/// be read declares nothing here.
pub(crate) fn unmet_need_now(
    store: &Store,
    plugin_store: &napp::plugin::PluginStore,
    agent_id: &str,
    binding_name: &str,
) -> Option<String> {
    let agent = store.get_agent(agent_id).ok().flatten()?;
    let config = napp::agent::parse_agent_config(&agent.frontmatter).ok()?;
    let binding = config.workflows.get(binding_name)?;
    let installed = agent::agent_worker::installed_interfaces(plugin_store);
    let enabled: Vec<String> = installed
        .iter()
        .map(|(slug, _)| slug)
        .filter(
            |slug| !matches!(store.get_plugin_by_slug(slug), Ok(Some(row)) if row.is_enabled == 0),
        )
        .cloned()
        .collect();
    unmet_need(store, &config, binding, &installed, &enabled)
}

/// Act on one fire's pre-flight. `unmet` None: the fire runs, and a need
/// recorded earlier is cleared. Some: the binding's record names the need
/// (said once at warn; later fires with the same need at debug), the fire
/// is closed `done` with the summary tag `skipped` (triage's history passes
/// over it) and does not run. The binding is never retired here. True runs.
///
/// `announce` is handed the need of every held fire; the owner hears it
/// once ([`db::Store::tell_binding_need`]), and clearing the record here
/// forgets it, so a need that returns is told again.
pub(crate) fn admit(
    store: &Store,
    run: &EngineRun,
    agent_id: &str,
    binding_name: &str,
    unmet: Option<String>,
    t: i64,
    announce: &dyn Fn(&str),
) -> bool {
    let recorded = store
        .agent_workflow_degraded_reason(agent_id, binding_name)
        .ok()
        .flatten()
        .unwrap_or_default();
    let key = format!("{agent_id}:{binding_name}");
    let Some(missing) = unmet else {
        if !recorded.is_empty() {
            info!(site = "preflight", binding = %key, was = %recorded, "binding's needs are present; running");
            let _ = store.set_agent_workflow_degraded_reason(agent_id, binding_name, "");
        }
        return true;
    };
    if recorded == missing {
        debug!(site = "preflight", binding = %key, missing = %missing, "binding needs are missing; fire held");
    } else {
        warn!(site = "preflight", binding = %key, missing = %missing, "binding needs are missing; fire held, binding kept");
        if let Err(e) = store.set_agent_workflow_degraded_reason(agent_id, binding_name, &missing) {
            warn!(site = "preflight", binding = %key, error = %e, "could not record the missing need");
        }
    }
    announce(&missing);
    if let Err(e) = store
        .engine_set_run_result_tag(&run.id, "skipped")
        .and_then(|_| store.engine_set_run_state(&run.id, "done", t, None))
    {
        warn!(site = "preflight", run = %run.id, error = %e, "could not close a held fire; running it");
        return true;
    }
    false
}

// ── Telling the owner ────────────────────────────────────────────────────

/// One Inbox item telling the owner an employee cannot do a duty until
/// something only the owner supplies is in place, and the one place to
/// supply it. Nothing is installed or connected for them: a connection can
/// provision something (a phone line, a paid account), so the owner decides.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NeedNotice {
    pub id: String,
    pub title: String,
    pub body: String,
    pub link: String,
}

/// A duty's name in plain words: `front-desk-report` → `front desk report`.
fn duty_words(binding_name: &str) -> String {
    binding_name.replace(['-', '_'], " ")
}

/// The notice for a need a binding's record names (`needs a telephony
/// plugin`, `needs the ledgerly plugin turned on`), from pre-flight or a
/// watch trigger's start. Each is met in Plugins, where a plugin is added
/// from the marketplace or turned on.
pub(crate) fn plugin_need_notice(employee: &str, binding_name: &str, need: &str) -> NeedNotice {
    NeedNotice {
        id: format!("need:{}", uuid::Uuid::new_v4()),
        title: format!("{employee} {need}"),
        body: format!(
            "{employee} is holding \"{duty}\" until then. Add the plugin or turn it on in Plugins. \
             The duty goes ahead on its own after that.",
            duty = duty_words(binding_name)
        ),
        link: crate::handlers::plugins::PLUGINS_SETTINGS_PATH.to_string(),
    }
}

/// The notice for a missing account on an installed plugin: open that
/// employee's accounts at that plugin. `plugin` is (slug, name).
pub(crate) fn account_need_notice(employee: &str, agent_id: &str, binding_name: &str, plugin: (&str, &str)) -> NeedNotice {
    let (slug, name) = plugin;
    NeedNotice {
        id: format!("need:{}", uuid::Uuid::new_v4()),
        title: format!("{employee} needs {name} connected"),
        body: format!(
            "{employee} can't do \"{duty}\" until a {name} account is connected for it. \
             Connect one in {employee}'s accounts. The duty goes ahead on its own after that.",
            duty = duty_words(binding_name)
        ),
        link: format!("/{agent_id}/settings/accounts?plugin={}", urlencoding::encode(slug)),
    }
}

/// The notice when a run's own words say the duty cannot be done until
/// something is connected, and what is not known. `clause` is the one short
/// piece of the run's words the owner is shown, quoted.
pub(crate) fn something_needed_notice(employee: &str, agent_id: &str, binding_name: &str, clause: &str) -> NeedNotice {
    let duty = duty_words(binding_name);
    let said = if clause.is_empty() { String::new() } else { format!(" Its last run said: \"{clause}\"") };
    NeedNotice {
        id: format!("need:{}", uuid::Uuid::new_v4()),
        title: format!("{employee} needs something connected"),
        body: format!(
            "{employee} can't do \"{duty}\" until something is added or connected.{said} \
             Check {employee}'s accounts and Plugins. The duty goes ahead on its own after that."
        ),
        link: format!("/{agent_id}/settings/accounts"),
    }
}

/// What a binding's duty stands on, from whichever source knows it.
pub(crate) enum Need<'a> {
    /// The reason the binding's record names (pre-flight, a watch trigger's
    /// start): `needs a telephony plugin`.
    Recorded(&'a str),
    /// What the tool that blocked a run named, as data.
    Known(&'a types::OwnerNeed),
    /// What heartbeat triage read the last outcome as standing on.
    Judged(&'a agent::heartbeat_triage::HeldNeed),
}

impl Need<'_> {
    /// The key "the owner was told this" is kept under.
    pub(crate) fn key(&self) -> String {
        use agent::heartbeat_triage::Declared;
        match self {
            Need::Recorded(text) => text.to_string(),
            Need::Known(need) => need.key(),
            Need::Judged(held) => match &held.which {
                Some(Declared::Capability(c)) => format!("judged:capability:{c}"),
                Some(Declared::Plugin(p)) => format!("judged:plugin:{p}"),
                None => "judged:something".to_string(),
            },
        }
    }
}

/// The owner need a binding's run ended blocked on, as the refusing tool
/// named it, or None: the run did not end blocked (`exited`), or the tool
/// named nothing the owner supplies.
pub(crate) fn blocked_need(store: &Store, run_id: &str) -> Option<types::OwnerNeed> {
    let run = store.get_workflow_run(run_id).ok().flatten()?;
    if run.status != "exited" {
        return None;
    }
    store.workflow_run_owner_need(run_id).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use db::NewRun;

    fn config(json: &str) -> napp::agent::AgentConfig {
        napp::agent::parse_agent_config(json).expect("config")
    }

    fn plugin(slug: &str, ifaces: &[&str]) -> (String, Vec<String>) {
        (
            slug.to_string(),
            ifaces.iter().map(|s| s.to_string()).collect(),
        )
    }

    const WATCHING: &str = r#"{"workflows":{"answer":{"trigger":{"type":"watch","plugin":"telephony","event":"call.incoming"},"activities":[{"id":"a","intent":"answer"}]}}}"#;
    const REQUIRING: &str = r#"{"requires":{"plugins":["@acme/plugins/ledgerly","PLUG-PJ3Z-ECFV"]},"workflows":{"sweep":{"trigger":{"type":"heartbeat","interval":"30m"},"activities":[{"id":"a","intent":"sweep"}]}}}"#;
    const NOTHING: &str = r#"{"requires":{"interfaces":["ledger","mail"]},"workflows":{"sweep":{"trigger":{"type":"heartbeat","interval":"30m"},"activities":[{"id":"a","intent":"sweep"}]}}}"#;

    #[test]
    fn a_watched_capability_needs_an_installed_plugin_that_binds_it() {
        let s = store();
        let c = config(WATCHING);
        let b = &c.workflows["answer"];
        let none = [plugin("sheets", &["spreadsheet"])];
        assert_eq!(
            unmet_need(&s, &c, b, &none, &["sheets".into()]).as_deref(),
            Some("needs a telephony plugin")
        );
        let one = [
            plugin("sheets", &["spreadsheet"]),
            plugin("voiceline", &["telephony"]),
        ];
        assert_eq!(unmet_need(&s, &c, b, &one, &[]), None);
        // A watch on a plugin named outright is that plugin's own business.
        let by_slug =
            config(&WATCHING.replace(r#""plugin":"telephony""#, r#""plugin":"voiceline""#));
        assert_eq!(
            unmet_need(&s, &by_slug, &by_slug.workflows["answer"], &one, &[]),
            None
        );
    }

    #[test]
    fn a_required_plugin_must_be_installed_and_turned_on() {
        let s = store();
        let c = config(&REQUIRING.replace(r#","PLUG-PJ3Z-ECFV""#, ""));
        let b = &c.workflows["sweep"];
        assert_eq!(
            unmet_need(&s, &c, b, &[], &[]).as_deref(),
            Some("needs the ledgerly plugin")
        );
        let installed = [plugin("ledgerly", &[])];
        assert_eq!(
            unmet_need(&s, &c, b, &installed, &[]).as_deref(),
            Some("needs the ledgerly plugin turned on")
        );
        assert_eq!(unmet_need(&s, &c, b, &installed, &["ledgerly".into()]), None);
    }

    /// A plugin required by its install code is the plugin that code
    /// installed here, held like one named by slug; a code nothing here was
    /// installed from is a need.
    #[test]
    fn a_plugin_required_by_its_install_code_is_checked() {
        let s = store();
        let c = config(REQUIRING);
        let b = &c.workflows["sweep"];
        let ledgerly = plugin("ledgerly", &[]);
        assert_eq!(
            unmet_need(&s, &c, b, &[ledgerly.clone()], &["ledgerly".into()]).as_deref(),
            Some("needs the plugin from code PLUG-PJ3Z-ECFV")
        );
        s.upsert_installed_plugin("coded-books", "Coded Books", "1.0.0", "", "", "", "")
            .unwrap();
        s.set_plugin_install_code("coded-books", "PLUG-PJ3Z-ECFV").unwrap();
        let both = [ledgerly, plugin("coded-books", &[])];
        assert_eq!(
            unmet_need(&s, &c, b, &both, &["ledgerly".into()]).as_deref(),
            Some("needs the coded-books plugin turned on")
        );
        assert_eq!(
            unmet_need(&s, &c, b, &both, &["ledgerly".into(), "coded-books".into()]),
            None
        );
        assert_eq!(required_plugin(&s, "PLUG-PJ3Z-ECFV"), "coded-books", "the engine's needs list names it too");
    }

    #[test]
    fn a_binding_that_declares_nothing_passes() {
        let s = store();
        let c = config(NOTHING);
        assert_eq!(
            unmet_need(&s, &c, &c.workflows["sweep"], &[], &[]),
            None,
            "requires.interfaces does not hold a binding"
        );
        let bare = config(
            r#"{"workflows":{"sweep":{"trigger":{"type":"schedule","cron":"0 0 9 * * *"}}}}"#,
        );
        assert_eq!(unmet_need(&s, &bare, &bare.workflows["sweep"], &[], &[]), None);
    }

    fn store() -> Store {
        let path = std::env::temp_dir().join(format!("nebo-preflight-{}.db", uuid::Uuid::new_v4()));
        Store::new(path.to_str().unwrap()).unwrap()
    }

    fn fire(s: &Store, id: &str) -> EngineRun {
        s.engine_create_run(&NewRun {
            id,
            kind: "task",
            session_key: "heartbeat-binding-emp-sweep",
            agent_id: "emp",
            lane: "main",
            inputs: Some(r#"{"command":"agent:emp:sweep","trigger":"heartbeat"}"#),
            external_ref: Some("hb:emp:sweep"),
            ..Default::default()
        })
        .unwrap();
        s.engine_get_run(id).unwrap().unwrap()
    }

    #[test]
    fn a_held_fire_records_the_need_keeps_the_binding_and_the_next_fire_runs_once_it_is_present() {
        let s = store();
        s.conn_exec_for_test(&format!(
            "INSERT INTO agents (id, name, description, agent_md, frontmatter, updated_at) VALUES ('emp', 'E', '', '', '{}', 0)",
            REQUIRING
        ));
        s.upsert_agent_workflow(
            "emp",
            "sweep",
            "heartbeat",
            "30m",
            None,
            None,
            None,
            None,
            None,
            false,
        )
        .unwrap();

        let first = fire(&s, "f1");
        assert_eq!(
            fire_binding(&s, &first),
            Some(("emp".into(), "sweep".into()))
        );
        assert!(!admit(
            &s,
            &first,
            "emp",
            "sweep",
            Some("needs the ledgerly plugin".into()),
            100,
            &|_| {}
        ));
        assert_eq!(
            s.agent_workflow_degraded_reason("emp", "sweep")
                .unwrap()
                .as_deref(),
            Some("needs the ledgerly plugin")
        );
        let closed = s.engine_get_run("f1").unwrap().unwrap();
        assert_eq!(
            (closed.state.as_str(), closed.summary.as_str()),
            ("done", "skipped")
        );
        assert!(
            s.is_agent_workflow_active("emp", "sweep").unwrap(),
            "held, never retired"
        );

        // The same need again: still held, record unchanged.
        let second = fire(&s, "f2");
        assert!(!admit(
            &s,
            &second,
            "emp",
            "sweep",
            Some("needs the ledgerly plugin".into()),
            200,
            &|_| {}
        ));

        // The plugin appears: the next fire runs and the record clears.
        let third = fire(&s, "f3");
        assert!(admit(&s, &third, "emp", "sweep", None, 300, &|_| {}));
        assert_eq!(
            s.agent_workflow_degraded_reason("emp", "sweep").unwrap(),
            None
        );
        assert_eq!(
            s.engine_get_run("f3").unwrap().unwrap().state,
            "queued",
            "an admitted fire is left to run"
        );
    }

    #[test]
    fn only_workflow_binding_fires_have_a_binding() {
        let s = store();
        let prompt = s
            .create_cron_job(
                "report",
                "0 0 9 * * *",
                "",
                "agent",
                Some("x"),
                None,
                None,
                true,
                Some("emp"),
                None,
                None,
            )
            .unwrap();
        let run = s
            .engine_get_run(&s.queue_cron_run(&prompt, false, false).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(fire_binding(&s, &run), None);
        let wf = s
            .create_cron_job(
                "agent-emp-sweep",
                "0 0 9 * * *",
                "agent:emp:sweep",
                "agent_workflow",
                None,
                None,
                None,
                true,
                Some("emp"),
                None,
                None,
            )
            .unwrap();
        let run = s
            .engine_get_run(&s.queue_cron_run(&wf, false, false).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(fire_binding(&s, &run), Some(("emp".into(), "sweep".into())));
    }
}
