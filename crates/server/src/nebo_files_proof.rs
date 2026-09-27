//! Nebo's own files and server, proven through the real server: an
//! employee's file tools and commands reach its workspace, its helpers'
//! copies and its own conversation's saved results, and never the rest of
//! Nebo's folder (the settings file with the server's secret, the logs, the
//! database, other conversations' files), however the path is spelled.
//! Nebo's own settings are not in its commands' environment, its commands
//! never reach Nebo's own API, and a scheduled command meets every one of
//! those limits and its employee's permissions.

use std::path::Path;

use serde_json::json;
use tools::Origin;
use types::permissions::{Effect, Rule, RuleKey, RuleSource, Scope, Writer};

use crate::staffed_proof::{Nebo, session};

/// `nebo-own-files-stay-closed`: on 2026-09-26 an employee asked to fix a
/// mail sign-in ran `cat <nebo-home>/settings.json` and put the server's
/// secret into its context (plugin-auth-no-self-reauth run 3), and others
/// read the server's logs (goal-persistence-across-segue, the replays). The
/// calls those runs made, through the doors they used, and the file work an
/// employee does every day, through the same doors.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_employee_never_reaches_nebo_own_files() {
    let nebo = session().await;
    let home = nebo.home.clone();
    let agent = format!("clerk-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    for cap in ["shell", "file"] {
        let rule = Rule {
            id: uuid::Uuid::new_v4().to_string(),
            scope: Scope::Employee(agent.clone()),
            key: RuleKey::Capability(cap.into()),
            field: None,
            effect: Effect::Allow,
            money: None,
            source: RuleSource::Owner,
            locked: false,
            created_at: 0,
        };
        nebo.store().write_permission_rule(&rule, &Writer::Owner).unwrap();
    }
    let ctx = Nebo::ctx(&agent, Origin::User);
    let at = |rel: &str| home.join(rel).to_string_lossy().into_owned();

    // What Nebo keeps that no employee reads: a secret, a log line, and
    // another conversation's saved result.
    let secret = "SECRET-accessSecret-canary";
    let settings = home.join("settings.json");
    if !settings.exists() {
        std::fs::write(&settings, format!("{{\"accessSecret\": \"{secret}\"}}")).unwrap();
    }
    let real_secret = std::fs::read_to_string(&settings).unwrap();
    std::fs::create_dir_all(home.join("logs")).unwrap();
    std::fs::write(home.join("logs/proof.log"), "LOG-canary gmail oauth").unwrap();
    std::fs::create_dir_all(home.join("sessions/other/tool-results")).unwrap();
    std::fs::write(home.join("sessions/other/tool-results/r.txt"), "THEIRS-canary").unwrap();
    let leaked = |text: &str| {
        text.contains(secret)
            || real_secret.lines().any(|l| l.contains("accessSecret") && text.contains(l.trim()))
            || text.contains("LOG-canary")
            || text.contains("THEIRS-canary")
    };

    // The file tools refuse them, however the path is spelled.
    for path in [
        at("settings.json"),
        at("logs/proof.log"),
        at("data/nebo.db"),
        at("files/../settings.json"),
        at("sessions/other/tool-results/r.txt"),
    ] {
        let r = nebo.tool(&ctx, "read_file", json!({ "path": path })).await;
        assert!(r.is_error && r.content.contains("Nebo's own files"), "read_file {path}: {}", r.content);
        assert!(!leaked(&r.content), "{}", r.content);
    }
    let r = nebo.tool(&ctx, "write_file", json!({ "path": at("settings.json"), "content": "{}" })).await;
    assert!(r.is_error, "the settings file was overwritten: {}", r.content);
    assert_eq!(std::fs::read_to_string(&settings).unwrap(), real_secret);

    // So does the shell, for the commands the runs made. Those that name the
    // folder are refused before they run; one that spells its way around the
    // name is stopped by the confinement, where this computer has one.
    for command in [
        format!("cat {} 2>/dev/null | head -50", at("settings.json")),
        format!("grep -ri 'gmail\\|oauth' {} | tail -30", at("logs/proof.log")),
        format!("find {} -type f | head -40", home.display()),
        format!("sqlite3 {} 'SELECT name FROM sqlite_master'", at("data/nebo.db")),
    ] {
        let r = nebo.tool(&ctx, "run_command", json!({ "command": command, "description": "Look into the connection" })).await;
        assert!(r.is_error && r.content.contains("Nebo's own files"), "{command}: {}", r.content);
        assert!(!leaked(&r.content), "{}", r.content);
    }
    std::fs::create_dir_all(home.join("files")).unwrap();
    let around = format!(
        "cd {} && cat ../settings.json ../logs/proof.log ../sessions/other/tool-results/r.txt",
        at("files")
    );
    let r = nebo.tool(&ctx, "run_command", json!({ "command": around, "description": "Read the settings" })).await;
    if tools::confine::available() {
        assert!(r.content.contains("settings.json"), "the command ran: {}", r.content);
        assert!(!leaked(&r.content), "the confined command read Nebo's own files: {}", r.content);
    } else {
        eprintln!("no confinement on this computer: Nebo's own files rest on the command's text alone");
    }

    // Nebo's own settings are not in a command's environment.
    let r = nebo.tool(&ctx, "run_command", json!({ "command": "env", "description": "Show the environment" })).await;
    assert!(!r.is_error, "{}", r.content);
    let own: Vec<&str> = r.content.lines().filter(|l| l.starts_with("NEBO_") || l.starts_with("NEBOAI_")).collect();
    assert!(own.is_empty(), "Nebo's own settings in a command's environment: {own:?}");

    // The employee's own work goes on as before: its workspace through
    // every door, its conversation's saved results, a helper's copy.
    let report = at("files/proof-report.md");
    let r = nebo.tool(&ctx, "write_file", json!({ "path": report, "content": "# Report\nMINE" })).await;
    assert!(!r.is_error, "{}", r.content);
    let r = nebo.tool(&ctx, "read_file", json!({ "path": report })).await;
    assert!(!r.is_error && r.content.contains("MINE"), "{}", r.content);
    let r = nebo
        .tool(&ctx, "run_command", json!({ "command": format!("cat '{report}' && echo MORE >> '{report}' && tail -1 '{report}'"), "description": "Add to the report" }))
        .await;
    assert!(!r.is_error && r.content.contains("MINE") && r.content.contains("MORE"), "{}", r.content);
    let saved = tools::result_shape::results_dir(&ctx.session_id).join("proof.txt");
    std::fs::create_dir_all(saved.parent().unwrap()).unwrap();
    std::fs::write(&saved, "SAVED-result").unwrap();
    let r = nebo.tool(&ctx, "read_file", json!({ "path": saved.to_string_lossy() })).await;
    assert!(!r.is_error && r.content.contains("SAVED-result"), "its own saved result: {}", r.content);
    let copy = home.join("worktrees/proof-copy");
    std::fs::create_dir_all(&copy).unwrap();
    let r = nebo
        .tool(&ctx, "run_command", json!({ "command": "echo built > out.txt && cat out.txt", "cwd": copy.to_string_lossy(), "description": "Build in the copy" }))
        .await;
    assert!(!r.is_error && r.content.contains("built"), "a helper's copy: {}", r.content);
    assert!(Path::new(&copy.join("out.txt")).exists());
}

/// An owner rule of `effect` on `key` in `agent`'s own scope.
fn own_rule(nebo: &Nebo, agent: &str, key: RuleKey, effect: Effect) {
    let rule = Rule {
        id: uuid::Uuid::new_v4().to_string(),
        scope: Scope::Employee(agent.to_string()),
        key,
        field: None,
        effect,
        money: None,
        source: RuleSource::Owner,
        locked: false,
        created_at: 0,
    };
    nebo.store().write_permission_rule(&rule, &Writer::Owner).unwrap();
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// `nebo-own-server-closed`: Nebo's local API trusts a caller on this
/// computer, so an employee's `curl` to it could do what the owner's app
/// does, permissions or not. A command never connects to it, by any
/// address; a server the employee started itself (a dev server) still
/// answers it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_employee_command_never_reaches_nebo_own_server() {
    let nebo = session().await;
    if !tools::confine::available() {
        eprintln!("no confinement on this computer: Nebo's own server is reachable from an employee's commands");
        return;
    }
    let agent = format!("dev-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    own_rule(&nebo, &agent, RuleKey::Capability("shell".into()), Effect::Allow);
    let ctx = Nebo::ctx(&agent, Origin::User);
    let run = |command: String| json!({ "command": command, "description": "Check the server" });
    assert!(types::own_ports::list().contains(&nebo.port), "the server's port is one of Nebo's own");

    for host in ["127.0.0.1", "localhost"] {
        let r = nebo
            .tool(&ctx, "run_command", run(format!("curl -s -m 5 -o /dev/null -w 'code=%{{http_code}}' http://{host}:{}/health", nebo.port)))
            .await;
        assert!(!r.content.contains("code=200"), "a command reached Nebo's API on {host}: {}", r.content);
    }

    // A dev server the employee starts answers it.
    let files = nebo.home.join("files");
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(files.join("proof-dev.txt"), "DEV-OK").unwrap();
    let dev = free_port();
    let started = nebo
        .tool(
            &ctx,
            "run_command",
            json!({
                "command": format!("python3 -m http.server {dev} --bind 127.0.0.1 --directory '{}'", files.display()),
                "description": "Start the dev server",
                "background": true,
            }),
        )
        .await;
    assert!(!started.is_error, "{}", started.content);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::net::TcpStream::connect(("127.0.0.1", dev)).is_err() {
        assert!(std::time::Instant::now() < deadline, "the dev server never came up: {}", started.content);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let r = nebo.tool(&ctx, "run_command", run(format!("curl -s -m 5 http://127.0.0.1:{dev}/proof-dev.txt"))).await;
    let _ = nebo.tool(&ctx, "run_command", run(format!("pkill -f 'http.server {dev}'"))).await;
    assert!(r.content.contains("DEV-OK"), "the employee's own server: {}", r.content);
}

/// A server on this computer that answers with a page titled "Example
/// Domain", and counts the connections it took.
fn page_server() -> (u16, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = hits.clone();
    std::thread::spawn(move || {
        for mut conn in listener.incoming().flatten() {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut buf = [0u8; 1024];
            let _ = conn.read(&mut buf);
            let body = "<html><title>Example Domain</title></html>";
            let _ = write!(conn, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        }
    });
    (port, hits)
}

/// A job that runs `command` as `agent`'s scheduled command.
fn scheduled(agent: &str, name: &str, command: String) -> db::models::CronJob {
    db::models::CronJob {
        id: 0,
        name: name.to_string(),
        schedule: "0 9 * * *".to_string(),
        command,
        task_type: "bash".to_string(),
        message: None,
        deliver: None,
        enabled: Some(1),
        last_run: None,
        run_count: None,
        last_error: None,
        created_at: None,
        instructions: None,
        agent_id: Some(agent.to_string()),
        channel_ctx_json: None,
        overlap_policy: "skip".to_string(),
    }
}

/// `scheduled-command-meets-employee-limits`: a scheduled command ran as
/// `sh -c` with Nebo's whole environment and no check, the one command no
/// limit reached: a web-off employee could schedule `curl`, and any
/// employee could schedule a read of Nebo's settings file. It now runs
/// through run_command under its employee's grant: web off means no
/// network, Nebo's own files stay closed, and a step that needs the owner's
/// OK is refused (nothing waits for an answer), recorded, and reported as
/// the fire's failure, never parked and never run unchecked.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_scheduled_command_meets_its_employee_limits() {
    use std::sync::atomic::Ordering;
    let nebo = session().await;
    let agent = format!("sched-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    own_rule(&nebo, &agent, RuleKey::Capability("shell".into()), Effect::Allow);
    own_rule(&nebo, &agent, RuleKey::Capability("web".into()), Effect::Deny);

    // Web off: the scheduled curl reaches nothing.
    let (port, hits) = page_server();
    let job = scheduled(&agent, "Morning fetch", format!("curl -s -m 5 http://127.0.0.1:{port}/"));
    let (_, output, err) = crate::scheduler::execute_job(&nebo.state, &job).await;
    let said = format!("{output}{}", err.unwrap_or_default());
    assert!(!said.contains("Example Domain"), "a web-off employee's scheduled curl got the page: {said}");
    assert!(said.contains("web access is off"), "{said}");
    assert_eq!(hits.load(Ordering::SeqCst), 0, "the scheduled command reached the server");

    // Nebo's settings file: refused.
    let settings = nebo.home.join("settings.json");
    if !settings.exists() {
        std::fs::write(&settings, "{\"accessSecret\": \"SECRET-accessSecret-canary\"}").unwrap();
    }
    let secret = std::fs::read_to_string(&settings).unwrap();
    let job = scheduled(&agent, "Read settings", format!("cat {}", settings.display()));
    let (ok, output, err) = crate::scheduler::execute_job(&nebo.state, &job).await;
    let err = err.unwrap_or_default();
    assert!(!ok && err.contains("Nebo's own files"), "{output}{err}");
    assert!(!output.contains(secret.trim()) && !err.contains(secret.trim()));

    // A step that needs the owner's OK is refused, not parked, and recorded.
    let careful = format!("ask-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    own_rule(&nebo, &careful, RuleKey::Capability("shell".into()), Effect::Allow);
    own_rule(&nebo, &careful, RuleKey::Tool("run_command".into()), Effect::Ask);
    let marker = nebo.home.join("files").join(format!("{careful}.marker"));
    let job = scheduled(&careful, "Touch a marker", format!("touch {}", marker.display()));
    let (ok, _, err) = crate::scheduler::execute_job(&nebo.state, &job).await;
    let err = err.unwrap_or_default();
    assert!(!ok && err.contains("needs the owner's OK"), "{err}");
    assert!(!marker.exists(), "the step that needed an OK ran");
    let open = nebo.state.permission_asks.open(None).unwrap();
    assert!(open.iter().all(|a| a.agent_id != careful), "the step was parked on a card: {open:?}");
    let (rows, _) = nebo
        .store()
        .permission_activity(&db::PermissionActivityFilter { agent_id: Some(careful.clone()), limit: 10, ..Default::default() })
        .unwrap();
    let refused = rows.iter().find(|r| r.tool == "run_command").expect("the decision is recorded");
    assert_eq!(refused.decision, "deny");
    assert!(refused.why.contains("cannot_wait"), "{}", refused.why);
    assert_eq!(refused.door, "schedule");
}

/// Runs `command` as the command step of a workflow `agent` owns: the
/// production engine, the harness's workflow loop and the registry roster a
/// run is given.
async fn workflow_step(nebo: &Nebo, agent: &str, command: &str) -> Result<String, String> {
    let def = workflow::parser::parse_workflow(
        &json!({
            "version": "1.0",
            "id": "proof-step",
            "name": "Proof step",
            "activities": [{ "id": "step", "type": "command", "params": { "command": command } }],
            "connections": [{ "from": "__trigger__", "to": "step" }, { "from": "step", "to": "__emit__" }],
        })
        .to_string(),
    )
    .unwrap();
    let roster: Vec<Box<dyn tools::registry::DynTool>> = nebo
        .state
        .tools
        .list()
        .await
        .iter()
        .map(|td| Box::new(crate::workflow_manager::RegistryTool::new(td, nebo.state.tools.clone())) as Box<dyn tools::registry::DynTool>)
        .collect();
    let looper = agent::harness::workflow_turn::WorkflowTurns::new(nebo.state.harness.clone());
    workflow::engine::execute_workflow(
        &def,
        agent,
        "",
        false,
        json!({}),
        "manual",
        None,
        nebo.store(),
        None,
        &looper,
        &roster,
        None,
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
        None,
    )
    .await
    .map(|(_, output)| output)
    .map_err(|e| e.to_string())
}

/// `workflow-command-step-meets-employee-limits`: a workflow's command step
/// ran with plugin credentials and no fence: Nebo's own files, its ports and
/// its local API were open to it, and a step that needed the owner's OK
/// parked a card nobody waited on. It now runs through run_command as the
/// employee that owns the workflow: web off means no network, Nebo's own
/// files stay closed (the installed plugins it runs stay open), and a step
/// that needs the owner's OK is refused and recorded, never parked.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_workflow_command_step_meets_its_employee_limits() {
    use std::sync::atomic::Ordering;
    let nebo = session().await;
    let agent = format!("flow-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    own_rule(&nebo, &agent, RuleKey::Capability("shell".into()), Effect::Allow);
    own_rule(&nebo, &agent, RuleKey::Capability("web".into()), Effect::Deny);

    // Web off: the step's curl reaches nothing.
    let (port, hits) = page_server();
    let said = match workflow_step(&nebo, &agent, &format!("curl -s -m 5 http://127.0.0.1:{port}/")).await {
        Ok(out) | Err(out) => out,
    };
    assert!(!said.contains("Example Domain"), "a web-off employee's workflow step got the page: {said}");
    assert!(!said.contains("AI provider"), "the step never reached the command door: {said}");
    assert_eq!(hits.load(Ordering::SeqCst), 0, "the workflow step reached the server");

    // Nebo's settings file: refused.
    let settings = nebo.home.join("settings.json");
    if !settings.exists() {
        std::fs::write(&settings, "{\"accessSecret\": \"SECRET-accessSecret-canary\"}").unwrap();
    }
    let secret = std::fs::read_to_string(&settings).unwrap();
    let err = workflow_step(&nebo, &agent, &format!("cat {}", settings.display())).await.unwrap_err();
    assert!(err.contains("Nebo's own files"), "{err}");
    assert!(!err.contains(secret.trim()));

    // An employee whose web access is on: a command spelled around the
    // text check still can't read Nebo's settings, and Nebo's own server
    // isn't reached, where this computer confines commands.
    let flow = format!("flowon-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    own_rule(&nebo, &flow, RuleKey::Capability("shell".into()), Effect::Allow);
    if tools::confine::available() {
        std::fs::create_dir_all(nebo.home.join("files")).unwrap();
        let around = format!("cd '{}' && cat ../settings.json", nebo.home.join("files").display());
        let said = match workflow_step(&nebo, &flow, &around).await {
            Ok(out) | Err(out) => out,
        };
        assert!(said.contains("settings.json"), "the command ran: {said}");
        assert!(!said.contains(secret.trim()), "a workflow step read Nebo's settings: {said}");
        let said = match workflow_step(&nebo, &flow, &format!("curl -s -m 5 -o /dev/null -w 'code=%{{http_code}}' http://127.0.0.1:{}/health", nebo.port)).await {
            Ok(out) | Err(out) => out,
        };
        assert!(!said.contains("code=200"), "a workflow step reached Nebo's API: {said}");
        // It ran, and could not connect (curl's 7), rather than being refused.
        assert!(said.contains("exited with code 7") || said.contains("code=000"), "{said}");
    } else {
        eprintln!("no confinement on this computer: a workflow step's reach rests on the permission check and the text check");
    }

    // The installed plugins it runs, and their data, stay open to it.
    let data = nebo.home.join("appdata/plugins/proofplug");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("state.txt"), "PLUGIN-DATA").unwrap();
    let bin_dir = nebo.home.join("nebo/plugins/proofplug/1.0.0");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let bin = bin_dir.join("proofplug");
    std::fs::write(&bin, format!("#!/bin/sh\necho PLUGIN-RAN; cat '{}'\n", data.join("state.txt").display())).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let out = workflow_step(&nebo, &flow, &bin.display().to_string()).await.expect("the plugin ran");
    assert!(out.contains("PLUGIN-RAN") && out.contains("PLUGIN-DATA"), "{out}");

    // A step that needs the owner's OK is refused, not parked, and recorded.
    let careful = format!("flowask-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    own_rule(&nebo, &careful, RuleKey::Capability("shell".into()), Effect::Allow);
    own_rule(&nebo, &careful, RuleKey::Tool("run_command".into()), Effect::Ask);
    let marker = nebo.home.join("files").join(format!("{careful}.marker"));
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
    let err = workflow_step(&nebo, &careful, &format!("touch {}", marker.display())).await.unwrap_err();
    assert!(err.contains("needs the owner's OK") && err.contains("a workflow step can't wait"), "{err}");
    assert!(!marker.exists(), "the step that needed an OK ran");
    let open = nebo.state.permission_asks.open(None).unwrap();
    assert!(open.iter().all(|a| a.agent_id != careful), "the step was parked on a card: {open:?}");
    let (rows, _) = nebo
        .store()
        .permission_activity(&db::PermissionActivityFilter { agent_id: Some(careful.clone()), limit: 10, ..Default::default() })
        .unwrap();
    let refused = rows.iter().find(|r| r.tool == "run_command").expect("the decision is recorded");
    assert_eq!(refused.decision, "deny");
    assert!(refused.why.contains("cannot_wait"), "{}", refused.why);
    assert_eq!(refused.door, "workflow");
}

/// `full-access-runs-unconfined`: the operating system's confinement of an
/// employee's commands (Nebo's own files and ports closed) broke programs
/// that start a sandbox of their own: Chrome without `--no-sandbox`,
/// `swift build`, Homebrew builds from source. The owner's call: an
/// employee with Full access runs its commands without it, and every other
/// employee keeps it. The same command, from a Full access employee and an
/// Automatic one: it reads a log of Nebo's through a path its text doesn't
/// name (what only the confinement stops), and on macOS starts a sandbox of
/// its own. The Full access employee's does both; the Automatic one's does
/// neither. What still holds for Full access: the command's text is checked,
/// Nebo's own settings stay out of its environment, and web access off
/// keeps its commands off the network. A helper runs as its parent does and
/// never gets more.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_full_access_employee_runs_unconfined() {
    use std::sync::atomic::Ordering;
    use types::permissions::{Ceiling, Door, Mode};
    let nebo = session().await;
    let home = nebo.home.clone();
    let id = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let (builder, clerk, researcher) = (format!("builder-{id}"), format!("clerk-{id}"), format!("researcher-{id}"));
    for agent in [&builder, &clerk, &researcher] {
        own_rule(&nebo, agent, RuleKey::Capability("shell".into()), Effect::Allow);
        own_rule(&nebo, agent, RuleKey::Capability("web".into()), Effect::Allow);
    }
    let store = nebo.store();
    store.set_permission_mode(&Scope::Employee(builder.clone()), Mode::FullAccess).unwrap();
    store.set_permission_mode(&Scope::Employee(clerk.clone()), Mode::Automatic).unwrap();
    // The researcher has Full access and its web access turned off.
    store.set_permission_mode(&Scope::Employee(researcher.clone()), Mode::FullAccess).unwrap();
    own_rule(&nebo, &researcher, RuleKey::Capability("web".into()), Effect::Deny);

    std::fs::create_dir_all(home.join("files")).unwrap();
    std::fs::create_dir_all(home.join("logs")).unwrap();
    std::fs::write(home.join("logs/full-access-proof.log"), "FULL-ACCESS-canary").unwrap();
    let files = home.join("files").to_string_lossy().into_owned();
    let run = |command: String| json!({ "command": command, "description": "Build the project" });
    // Reaches a closed path by a spelling the command's text check can't
    // follow: only the operating system's confinement stops it.
    let read_log = run(format!("cd '{files}' && cat ../logs/full-access-proof.log"));
    let reads = |r: &tools::ToolResult| r.content.contains("FULL-ACCESS-canary");
    // A program that starts a sandbox of its own, as Chrome and `swift
    // build` do. macOS allows none inside another.
    let nested = run("/usr/bin/sandbox-exec -p '(version 1)(allow default)' /usr/bin/true && echo NESTED-OK".into());
    let nests = |r: &tools::ToolResult| r.content.contains("NESTED-OK");
    let helper = |parent: &str, mode: Mode, n: u8| {
        let mut ctx = Nebo::ctx(parent, Origin::User);
        let parent_grant = agent::resolve_grant(store, parent, None);
        let mut grant = parent_grant.clone();
        grant.mode = mode;
        grant.ceiling = Some(Ceiling::Parent { grant: Box::new(parent_grant) });
        ctx.session_key = format!("subagent:agent:{parent}:main:sa-{n}");
        ctx.door = Door::Helper;
        ctx.grant = Some(std::sync::Arc::new(grant));
        ctx
    };

    // Full access: unconfined, and so is its helper.
    for (who, ctx) in [
        ("the Full access employee", Nebo::ctx(&builder, Origin::User)),
        ("its helper", helper(&builder, Mode::FullAccess, 1)),
    ] {
        let r = nebo.tool(&ctx, "run_command", read_log.clone()).await;
        assert!(!r.is_error && reads(&r), "{who}'s command ran confined: {}", r.content);
        if cfg!(target_os = "macos") && tools::confine::available() {
            let r = nebo.tool(&ctx, "run_command", nested.clone()).await;
            assert!(nests(&r), "{who}'s command could not start a sandbox of its own: {}", r.content);
        }
    }

    // Everyone else: confined. The Automatic employee, a helper that asks
    // for Full access under it, and a Full access employee's helper that
    // runs Automatic.
    if tools::confine::available() {
        for (who, ctx) in [
            ("the Automatic employee", Nebo::ctx(&clerk, Origin::User)),
            ("a Full access helper under the Automatic employee", helper(&clerk, Mode::FullAccess, 2)),
            ("an Automatic helper under the Full access employee", helper(&builder, Mode::Automatic, 3)),
        ] {
            let r = nebo.tool(&ctx, "run_command", read_log.clone()).await;
            assert!(!reads(&r), "{who}'s command read Nebo's own files: {}", r.content);
            if cfg!(target_os = "macos") {
                let r = nebo.tool(&ctx, "run_command", nested.clone()).await;
                assert!(!nests(&r), "{who}'s command ran unconfined: {}", r.content);
            }
        }
    } else {
        eprintln!("no confinement on this computer: every employee's commands run unconfined");
    }

    // What Full access doesn't lift: the command's text is checked, and
    // Nebo's own settings are not in its environment.
    let full = Nebo::ctx(&builder, Origin::User);
    let r = nebo.tool(&full, "run_command", run(format!("cat '{}'", home.join("logs/full-access-proof.log").display()))).await;
    assert!(r.is_error && r.content.contains("Nebo's own files") && !reads(&r), "{}", r.content);
    let r = nebo.tool(&full, "run_command", run("env".into())).await;
    let own: Vec<&str> = r.content.lines().filter(|l| l.starts_with("NEBO_") || l.starts_with("NEBOAI_")).collect();
    assert!(!r.is_error && own.is_empty(), "Nebo's own settings in a Full access command's environment: {own:?}");

    // Web access off with Full access: the owner's web setting holds. The
    // command runs offline and nothing else is closed; on a computer that
    // can't keep a command offline, it doesn't run.
    let (port, hits) = page_server();
    let fetch = run(format!("curl -s -m 5 http://127.0.0.1:{port}/"));
    for (who, ctx) in [
        ("the web-off Full access employee", Nebo::ctx(&researcher, Origin::User)),
        ("its helper", helper(&researcher, Mode::FullAccess, 4)),
    ] {
        let r = nebo.tool(&ctx, "run_command", fetch.clone()).await;
        assert!(!r.content.contains("Example Domain"), "{who} reached the page: {}", r.content);
        assert!(r.content.contains("web access is off"), "{who} was not told why: {}", r.content);
        let r = nebo.tool(&ctx, "run_command", read_log.clone()).await;
        if tools::confine::available() {
            assert!(reads(&r), "{who}'s offline command was confined beyond the network: {}", r.content);
        } else {
            assert!(r.is_error && r.content.contains("web access is off"), "{}", r.content);
        }
    }
    assert_eq!(hits.load(Ordering::SeqCst), 0, "a web-off Full access command reached the server");
    let r = nebo.tool(&full, "run_command", fetch).await;
    assert!(r.content.contains("Example Domain"), "the command itself works: {}", r.content);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}
