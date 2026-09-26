//! Nebo's own files, proven through the real server: an employee's file
//! tools and commands reach its workspace, its helpers' copies and its own
//! conversation's saved results, and never the rest of Nebo's folder (the
//! settings file with the server's secret, the logs, the database, other
//! conversations' files), however the path is spelled. Nebo's own settings
//! are not in its commands' environment either.

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
