//! An app runs a plugin directly, proven through the real server: the
//! app's sidecar calls `POST /api/v1/apps/<id>/plugins/<slug>/run` with its
//! `NEBO_APP_TOKEN` and gets what the employee's `plugin__<slug>` tool gets,
//! from the same plugin run the same way (the same account, and the same
//! credential for the plugin's metered calls). A plugin the manifest does
//! not grant (or a command its grant does not cover), one that is not
//! installed, another app's token or pass, and an installed app the owner
//! has not allowed (until he allows it) are each refused. The app's page
//! runs it with its pass. The plugins' local credential never comes back to
//! the app. A long job reports its progress as it goes.

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::sidecar_proof::{World, quick};
use crate::staffed_proof::{FAKE_LEDGER, Nebo, session};

/// A plugin that says who ran it and with what, and draws three frames,
/// slowly, when asked to.
const SCRIPT: &str = "#!/bin/sh\n\
case \"$1\" in\n\
  frames) for i in 1 2 3; do echo \"frame $i of 3\"; sleep 0.4; done; echo finished ;;\n\
  *) echo \"agent=$NEBO_AGENT_ID\"; echo \"local=$NEBO_LOCAL_URL\"; printf '%s\\n' \"$@\" ;;\n\
esac\n";

fn install_plugin(home: &Path, slug: &str) {
    let dir = home.join("user").join("plugins").join(slug).join("0.1.0");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = dir.join(slug);
    std::fs::write(&bin, SCRIPT).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut platforms = serde_json::Map::new();
    platforms.insert(
        napp::plugin::current_platform_key(),
        json!({"binaryName": slug, "sha256": "", "signature": "", "size": SCRIPT.len(), "downloadUrl": ""}),
    );
    // One of its commands is a gated operation of the owner's (a payment):
    // a raw run of it is refused and pointed at its operation tool.
    let manifest = json!({
        "id": slug, "slug": slug, "name": "Frames", "version": "0.1.0", "platforms": platforms,
        "interfaceBindings": {"ledger.payment.apply": "payment create --customer-ref {customerRef} --total-amt {totalAmt}"},
    });
    std::fs::write(dir.join("plugin.json"), manifest.to_string()).unwrap();
}

/// An app package under `root` (`user/agents` for one made here,
/// `nebo/agents` for one installed from the marketplace), known to the
/// loader and the database as an app.
async fn place_app(nebo: &Nebo, root: &str, id: &str, name: &str, permissions: &[String]) {
    let dir = nebo.home.join(root).join(id);
    std::fs::create_dir_all(dir.join("ui")).unwrap();
    std::fs::write(dir.join("AGENT.md"), format!("---\nname: {name}\ndescription: {name}, an app.\n---\n\n# {name}\n")).unwrap();
    std::fs::write(dir.join("ui/index.html"), "<!doctype html><title>app</title>").unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        json!({"id": id, "name": name, "version": "1.0.0", "type": "app", "permissions": permissions}).to_string(),
    )
    .unwrap();
    let store = nebo.store();
    let kind = if root.starts_with("user") { "user" } else { "installed" };
    if store.get_agent(id).unwrap().is_none() {
        let _ = store.create_agent(id, Some(kind), name, "", "", "{}", None, None);
    }
    store.set_agent_napp_path(id, &dir.to_string_lossy()).unwrap();
    // Where it came from is recorded on its row: built here, or installed.
    store.set_agent_built_here(id, root.starts_with("user")).unwrap();
    store.set_agent_app_fields(id, true, Some(&dir.join("ui").to_string_lossy()), None, None).unwrap();
    nebo.state.agent_loader.load_all().await;
    assert!(
        nebo.state.agent_loader.list().await.iter().any(|a| a.is_app && a.id.as_deref() == Some(id)),
        "the loader holds {id}"
    );
}

/// A running sidecar for `id`, whose launch token its calls carry.
async fn sidecar(nebo: &Nebo, id: &str) -> (World, String) {
    let world = World::new(napp::test_sidecar::TestApp::new(id), quick(), |_| {}).await;
    world.until(20, |s| matches!(s, napp::supervisor::SidecarState::Running(_))).await;
    nebo.state.app_lifecycles.write().await.insert(id.to_string(), world.lifecycle.clone());
    let token = world.lifecycle.app_token().await;
    (world, token)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_runs_a_granted_plugin_as_its_employee_would_and_nothing_else() {
    let nebo = session().await;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..6];
    let slug = format!("frames-{tag}");
    let tool = format!("plugin__{slug}");
    install_plugin(&nebo.home, &slug);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while nebo.state.tools.get(&tool).await.is_none() {
        assert!(tokio::time::Instant::now() < deadline, "{slug} was not loaded");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let own = format!("studio{tag}");
    let missing = format!("absent-{tag}");
    place_app(&nebo, "user/agents", &own, "Studio", &[format!("plugin:{slug}"), format!("plugin:{missing}")]).await;
    let bought = format!("bought{tag}");
    place_app(&nebo, "nebo/agents", &bought, "Bought Studio", &[format!("plugin:{slug}")]).await;
    let (own_world, own_token) = sidecar(&nebo, &own).await;
    let (bought_world, bought_token) = sidecar(&nebo, &bought).await;

    // The sidecar's own client: no install key, only its launch token.
    let client = tls::http_client().pool_max_idle_per_host(0).build().unwrap();
    let base = format!("http://127.0.0.1:{}/api/v1/apps", nebo.port);
    let run = |app: &str, plugin: &str, token: &str, body: Value| {
        client.post(format!("{base}/{app}/plugins/{plugin}/run")).bearer_auth(token).json(&body).send()
    };
    let call = json!({"command": "draw --fast", "args": {"title": "Opening shot"}});

    // A granted plugin: the employee's tool's result, from the same run.
    let r = run(&own, &slug, &own_token, call.clone()).await.unwrap();
    assert_eq!(r.status(), 200);
    let out: Value = r.json().await.unwrap();
    assert_eq!(out["ok"], true, "{out}");
    let output = out["output"].as_str().unwrap();
    assert!(output.contains(&format!("agent={own}")), "the app's employee's account: {output}");
    // The plugin's metered calls go through Nebo's plugin routes, and the
    // credential they carry never reaches the app.
    let local = format!("local=http://127.0.0.1:{}/k/<credential>", nebo.port);
    assert!(output.contains(&local), "{output}");
    assert!(!output.contains(napp::plugin::plugin_local_token()), "the plugins' credential never reaches an app: {output}");
    assert!(output.contains("draw\n--fast\n--title\nOpening shot"), "{output}");
    let employee = Nebo::ctx(&own, tools::Origin::User);
    let by_employee = nebo.state.tools.execute(&employee, &tool, call.clone()).await;
    assert!(!by_employee.is_error, "{}", by_employee.content);
    assert_eq!(output, by_employee.content, "the same result the employee's tool gets, through the same door");

    // Read the way the runner reads it: a leading plugin name and quotes
    // run the same command. An app names no other employee: the field is
    // not its to give, and the run stays its own employee's.
    let r = run(&own, &slug, &own_token, json!({"command": format!("\"{slug}\" draw"), "employee": "someone-else"})).await.unwrap();
    let out: Value = r.json().await.unwrap();
    assert!(out["output"].as_str().unwrap().contains(&format!("agent={own}")), "{out}");
    let r = run(&own, &slug, &own_token, json!({"command": "draw \"open"})).await.unwrap();
    assert_eq!(r.status(), 400, "an unclosed quote");

    // The owner's gated operation, however the command is written, is
    // refused as it is for his employee, and never runs.
    for gated in ["payment create --total-amt 5".to_string(), format!("\"{slug}\"  payment   \"create\"")] {
        let out: Value = run(&own, &slug, &own_token, json!({"command": gated})).await.unwrap().json().await.unwrap();
        assert_eq!(out["ok"], false, "{gated}: {out}");
        let said = out["output"].as_str().unwrap();
        assert!(said.contains("ledger.payment.apply") && !said.contains("agent="), "{gated}: {said}");
    }

    // Refused: a plugin the manifest does not grant, a granted one that is
    // not installed, another app's token, no token.
    let r = run(&own, FAKE_LEDGER, &own_token, call.clone()).await.unwrap();
    assert_eq!(r.status(), 403, "{}", r.text().await.unwrap());
    let r = run(&own, &missing, &own_token, call.clone()).await.unwrap();
    assert_eq!(r.status(), 404, "{}", r.text().await.unwrap());
    let r = run(&own, &slug, &bought_token, call.clone()).await.unwrap();
    assert_eq!(r.status(), 401, "another app's token");
    let r = client.post(format!("{base}/{own}/plugins/{slug}/run")).json(&call).send().await.unwrap();
    assert_eq!(r.status(), 401, "no token");

    // An app installed from the marketplace declares the grant, and the
    // owner has not allowed it.
    let r = run(&bought, &slug, &bought_token, call.clone()).await.unwrap();
    assert_eq!(r.status(), 403);
    let why = r.text().await.unwrap();
    assert!(why.contains(&format!("Bought Studio wants to use {slug}")), "{why}");
    // The owner sees the request in those words, allows it, and the call
    // runs; he takes it back, and it is refused again.
    let page = crate::app_grants::listing_for(&nebo.state, &bought).await.unwrap();
    let asked = page.permissions.iter().find(|p| p.permission == format!("plugin:{slug}")).expect("the plugin request is listed");
    assert_eq!((asked.kind.as_str(), asked.target.as_str(), asked.state.as_str()), ("plugin", "Frames", "asking"));
    crate::app_grants::answer(&nebo.state, &bought, &format!("plugin:{slug}"), true).await.unwrap();
    let r = run(&bought, &slug, &bought_token, call.clone()).await.unwrap();
    assert_eq!(r.status(), 200, "allowed: {}", r.text().await.unwrap());
    crate::app_grants::answer(&nebo.state, &bought, &format!("plugin:{slug}"), false).await.unwrap();
    assert_eq!(run(&bought, &slug, &bought_token, call.clone()).await.unwrap().status(), 403, "taken back");

    // The app's page: its own pass, on its own route, from an opaque origin.
    let pass = napp::page_pass::grant(napp::page_pass::Scope::App(own.clone()), Duration::from_secs(60));
    let paged = format!("http://127.0.0.1:{}/k/{pass}/api/v1/apps", nebo.port);
    let r = client.post(format!("{paged}/{own}/plugins/{slug}/run")).header("origin", "null").json(&call).send().await.unwrap();
    assert_eq!(r.status(), 200, "the page's pass: {}", r.text().await.unwrap());
    let r = client.post(format!("{paged}/{bought}/plugins/{slug}/run")).header("origin", "null").json(&call).send().await.unwrap();
    assert_eq!(r.status(), 401, "one app's pass never runs another app's plugins");

    // A grant for one command: that command runs, another is refused.
    let scoped = format!("scoped{tag}");
    place_app(&nebo, "user/agents", &scoped, "Scoped Studio", &[format!("plugin:{slug}:draw")]).await;
    let pass = napp::page_pass::grant(napp::page_pass::Scope::App(scoped.clone()), Duration::from_secs(60));
    let scoped_run = |body: Value| {
        client
            .post(format!("http://127.0.0.1:{}/k/{pass}/api/v1/apps/{scoped}/plugins/{slug}/run", nebo.port))
            .header("origin", "null")
            .json(&body)
            .send()
    };
    assert_eq!(scoped_run(json!({"command": "draw"})).await.unwrap().status(), 200, "the granted command");
    assert_eq!(scoped_run(json!({"command": format!("{slug} draw")})).await.unwrap().status(), 200, "named with the plugin");
    let r = scoped_run(json!({"command": "frames"})).await.unwrap();
    assert_eq!(r.status(), 403, "another command: {}", r.text().await.unwrap());

    // A long job answers at once and reports each frame as it is drawn.
    let r = run(&own, &slug, &own_token, json!({"command": "frames", "background": true})).await.unwrap();
    assert_eq!(r.status(), 202);
    let job = r.json::<Value>().await.unwrap()["job"].as_str().unwrap().to_string();
    let jobs = format!("{base}/{own}/plugins/jobs/{job}");
    let mut stream = client.get(&jobs).bearer_auth(&own_token).header("accept", "text/event-stream").send().await.unwrap();
    assert_eq!(stream.status(), 200);
    let mut text = String::new();
    let mut first_frame_while_running = None;
    while let Some(chunk) = tokio::time::timeout(Duration::from_secs(20), stream.chunk()).await.expect("the stream ends").unwrap() {
        text.push_str(&String::from_utf8_lossy(&chunk));
        if first_frame_while_running.is_none() && text.contains("frame 1 of 3") {
            let snap: Value = client.get(&jobs).bearer_auth(&own_token).send().await.unwrap().json().await.unwrap();
            first_frame_while_running = Some(snap["done"] == false);
        }
    }
    assert_eq!(first_frame_while_running, Some(true), "progress arrives while the job runs: {text}");
    let frames: Vec<&str> = text.lines().filter_map(|l| l.strip_prefix("data: ")).collect();
    assert_eq!(&frames[..4], ["frame 1 of 3", "frame 2 of 3", "frame 3 of 3", "finished"], "{text}");
    let result: Value = serde_json::from_str(frames.last().unwrap()).unwrap();
    assert_eq!(result["ok"], true);
    assert!(text.contains("event: progress") && text.contains("event: result"), "{text}");
    let snap: Value = client.get(&jobs).bearer_auth(&own_token).send().await.unwrap().json().await.unwrap();
    assert_eq!(snap["done"], true);
    assert!(snap["result"]["output"].as_str().unwrap().contains("frame 3 of 3\nfinished"), "{snap}");
    let r = client.get(&jobs).bearer_auth(&bought_token).send().await.unwrap();
    assert_eq!(r.status(), 401, "another app's token");
    let r = client.get(format!("{base}/{bought}/plugins/jobs/{job}")).bearer_auth(&bought_token).send().await.unwrap();
    assert_eq!(r.status(), 404, "another app's job is no job of its own");

    for (id, world) in [(own, own_world), (bought, bought_world)] {
        nebo.state.app_lifecycles.write().await.remove(&id);
        world.lifecycle.shutdown().await;
    }
}
