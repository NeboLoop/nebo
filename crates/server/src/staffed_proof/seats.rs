//! The seats: out-of-bounds work is an assignment, a package's own skills
//! load scoped to it, the owner's declaration survives a package update, the
//! department locks and the reporting line refuses a loop.

use super::*;
use tools::Origin;

/// Out-of-bounds work is somebody's job. The join itself — the runner's gate
/// deciding Approval in an unattended run and handing the work on — sits
/// inside a model turn and cannot be driven without a model (it is held by
/// `runner::out_of_bounds_tests`). What the server exposes is proven here:
/// the ONE hand-over pathway is installed at boot and works end to end (the
/// `agent` tool's assign, through the opener, opens the assignee's own case
/// with a first turn and a durable assignment row carrying who asked, what,
/// and what done means), and the two facts the join reads to find the
/// authority seat are on the row and on the policy as the owner wrote them:
/// a seat granted `authority.grant.grant`, and a reporting line the owner
/// drew. With neither, nothing here names a manager, and the caller's own
/// fallback — park for the owner — stands.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn out_of_bounds_work_becomes_an_assignment() {
    let nebo = session().await;
    let bk = nebo.hire("Ledger Clerk", json!({ "requires": { "interfaces": ["ledger"] }, "workflows": {} })).await;
    let lead = nebo.hire("Operations Lead", json!({ "workflows": {} })).await;
    let om = nebo.hire("Front Office", json!({ "workflows": {} })).await;

    // Nobody holds authority and no line is drawn: the row and the policies say so.
    assert!(nebo.agent(&bk).reports_to.is_none());
    for id in [&bk, &lead, &om] {
        assert!(!nebo.operation_rules(id).contains_key("authority.grant.grant"));
    }
    assert!(tools::assignments::assignment_opener().is_some(), "the server installed the opener at boot");

    // The owner gives the lead the authority to grant.
    nebo.put_ok(
        &format!("/entity-config/agent/{lead}"),
        &json!({ "operationPolicy": { "operations": { "authority.grant.grant": "approval" } } }),
    )
    .await;
    let holder = nebo.operation_rules(&lead);
    assert_ne!(holder["authority.grant.grant"].effect, types::permissions::Effect::Deny);

    // The hand-over, as the runner does it: the assign action from an
    // unattended run of the bookkeeper's, carrying the operation, the asking
    // seat, the bound and the stopped work.
    let ctx = tools::ToolContext::new(Origin::System).with_session(format!("agent:{bk}:cron"), "s1");
    const OP: &str = "ledger.billpayment.create";
    const DISPLAY: &str = "Pay the roofing supplier for bill 1042";
    const REASON: &str = "amount 300000 exceeds the grant's 250000 per operation";
    let handed = nebo
        .tool(
            &ctx,
            "assign_task",
            json!({
                "to": "Operations Lead",
                "subject": format!("Ledger Clerk is stopped on {OP}: {DISPLAY}"),
                "done_means": format!("Decide {OP} for Ledger Clerk. It fell outside what Ledger Clerk may do unattended: {REASON}. The work that is stopped: {DISPLAY}."),
            }),
        )
        .await;
    assert!(!handed.is_error, "{}", handed.content);
    assert!(handed.content.contains("Assigned to Operations Lead as their own work"), "{}", handed.content);
    assert!(handed.content.contains("do not do it yourself"), "{}", handed.content);

    // The assignment row: who, what, what done means.
    let aid = handed.content.split("(assignment ").nth(1).unwrap().split(')').next().unwrap().to_string();
    for i in 0..6 {
        let by_id = nebo.store().get_assignment(&aid).unwrap().map(|a| a.state.clone());
        let listed = nebo.store().list_assignments_for_agent(&lead, true).unwrap().len();
        let all = nebo.store().list_assignments_for_agent(&lead, false).unwrap().len();
        let fresh = db::Store::new(&nebo.home.join("data").join("nebo.db").to_string_lossy()).unwrap().list_assignments_for_agent(&lead, true).unwrap().len();
        eprintln!("DEBUG probe {i}: by_id={by_id:?} listed_open={listed} listed_all={all} fresh_store_open={fresh}");
    }
    let open = nebo.store().list_assignments_for_agent(&lead, true).unwrap();
    assert_eq!(open.len(), 1, "one assignment on the lead: {open:?}");
    let a = &open[0];
    assert_eq!(a.assigner_agent_id, bk);
    assert_eq!(a.assigner_session_key, format!("agent:{bk}:cron"));
    assert_eq!(a.state, "open");
    assert!(a.subject.contains("Ledger Clerk") && a.subject.contains(OP) && a.subject.contains(DISPLAY), "{}", a.subject);
    assert!(a.done_means.contains(REASON) && a.done_means.contains(OP), "{}", a.done_means);
    // The assignee's own case, keyed on the assignment, with its first turn.
    let case = nebo.store().engine_run_for_key("case:assignment", &a.id).unwrap().expect("the case is bound");
    assert_eq!(case.agent_id, lead);
    assert_eq!(case.kind, "case");
    let turns = nebo.store().engine_children(&case.id).unwrap();
    assert_eq!(turns.len(), 1, "one first turn");
    let inputs: Value = serde_json::from_str(turns[0].inputs.as_deref().unwrap()).unwrap();
    assert_eq!(inputs["_assignment"]["id"], a.id);
    assert_eq!(inputs["_assignment"]["assigner_agent_id"], bk);
    // The bookkeeper sees the same one row as its assigner, and holds no assignment of its own.
    let bk_side = nebo.store().list_assignments_for_agent(&bk, true).unwrap();
    assert_eq!(bk_side.len(), 1);
    assert_eq!(bk_side[0].id, a.id);
    assert!(bk_side.iter().all(|x| x.assignee_agent_id != bk), "nothing landed on the bookkeeper as assignee");

    // The line the owner draws wins over the search, and it is on the row.
    nebo.put_ok(&format!("/agents/{bk}"), &json!({ "reportsTo": om })).await;
    assert_eq!(nebo.agent(&bk).reports_to.as_deref(), Some(om.as_str()));
    let roster = nebo.get_ok("/agents").await;
    let row = roster["agents"].as_array().unwrap().iter().find(|a| a["id"] == bk).unwrap();
    assert_eq!(row["reportsTo"], om, "{row}");
    assert_eq!(nebo.store().manager_chain(&bk).unwrap(), vec![(om.clone(), "Front Office".to_string())]);
    // A seat may not assign to itself.
    let me = nebo
        .tool(&ctx, "assign_task", json!({ "to": "Ledger Clerk", "subject": "x", "done_means": "y" }))
        .await;
    assert!(me.is_error && me.content.contains("That is you"), "{}", me.content);
}

/// Two employee packages ship a procedure under the same name; a third
/// declares it and ships nothing. After the real server's first scan: each
/// of the two sees its own through the loader the runner uses, the third
/// sees neither, the shared roster (the `/skills` door) knows no such skill,
/// and the two packaged employees read as healthy while the third is
/// degraded for the skill it does not ship.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_packages_own_skills_load_scoped() {
    // The three packages were written before the server's first scan
    // (`prepare_all`): two ship the procedure, the third only declares it.
    let nebo = session().await;
    for id in ["copywriter", "closer", "greeter"] {
        assert_eq!(nebo.agent(id).name.to_lowercase(), id, "the package was hired at boot");
    }

    let loader = &nebo.state.skill_loader;
    let mine = loader.get(SKILL, Some("copywriter")).await.expect("the copywriter sees its own");
    assert_eq!(mine.description, "How the copywriter works");
    assert_eq!(mine.owner_agent_id.as_deref(), Some("copywriter"));
    let theirs = loader.get(SKILL, Some("closer")).await.expect("the closer sees its own");
    assert_eq!(theirs.description, "How the closer works");
    assert_eq!(theirs.owner_agent_id.as_deref(), Some("closer"));
    assert!(loader.get(SKILL, Some("greeter")).await.is_none(), "a third seat sees neither");
    assert!(loader.get(SKILL, None).await.is_none(), "nothing leaked onto the shared roster");
    let (status, _) = nebo.get(&format!("/skills/{SKILL}")).await;
    assert_eq!(status, 404, "the shared door knows no such skill");
    let extensions = nebo.get_ok("/extensions").await;
    assert!(!extensions.to_string().contains("How the copywriter works"), "{extensions}");

    // Healthy, not degraded: the dependency check is scoped to the seat.
    let registry = nebo.state.agent_registry.read().await;
    assert_eq!(registry.get("copywriter").expect("active").degraded, None);
    assert_eq!(registry.get("closer").expect("active").degraded, None);
    let greeter = registry.get("greeter").expect("active").degraded.clone().expect("degraded");
    assert!(greeter.contains("missing skills") && greeter.contains(SKILL), "{greeter}");
}

/// The owner adds a capability and a question to a packaged employee. The
/// package then updates with a new question and a corrected label. Both
/// owner edits survive and both package changes arrive: the merge is per
/// entry against the baseline the package last delivered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_owners_declaration_survives_a_package_update() {
    // The package was hired at the server's first scan (`prepare_all`).
    const ID: &str = PAYABLES;
    let nebo = session().await;
    let before = nebo.get_ok(&format!("/agents/{ID}")).await;
    assert_eq!(before["interfaces"], json!(["ledger"]), "{before}");

    // The owner: a capability, and a question of their own.
    nebo.put_ok(
        &format!("/agents/{ID}"),
        &json!({
            "interfaces": ["ledger", "mail"],
            "inputs": [
                { "key": "invoice_mailbox", "id": "finance.ap.invoice_mailbox", "label": "Which mailbox?", "type": "text" },
                { "key": "po_prefix", "id": "finance.ap.po_prefix", "label": "What prefix do purchase orders carry?", "type": "text" }
            ]
        }),
    )
    .await;
    let edited = nebo.get_ok(&format!("/agents/{ID}")).await;
    assert_eq!(edited["interfaces"], json!(["ledger", "mail"]), "{edited}");
    let fm: Value = serde_json::from_str(&nebo.agent(ID).frontmatter).unwrap();
    assert!(fm.get(db::declaration::PACKAGE_BASELINE).is_some(), "the baseline was recorded at the owner's first edit: {fm}");

    // The package updates: a corrected label, a new question, still the ledger only.
    let v2 = json!({
        "requires": { "interfaces": ["ledger"] },
        "inputs": [
            { "key": "invoice_mailbox", "id": "finance.ap.invoice_mailbox", "label": "Which mailbox do bills arrive in?", "type": "text" },
            { "key": "approver", "id": "finance.ap.approver", "label": "Who approves a bill over the ceiling?", "type": "text" }
        ],
        "workflows": {}
    });
    let pkg = nebo.home.join("user").join("agents").join(ID);
    std::fs::write(pkg.join("agent.json"), serde_json::to_string_pretty(&v2).unwrap()).unwrap();
    let reloaded = nebo.post_ok(&format!("/agents/{ID}/reload"), &json!({})).await;
    assert_eq!(reloaded["ok"], true, "{reloaded}");
    assert!(reloaded["reloaded"].as_array().unwrap().iter().any(|c| c == "agent.json"), "{reloaded}");

    let after = nebo.get_ok(&format!("/agents/{ID}")).await;
    let agent = &after;
    // Both owner edits survive.
    assert_eq!(agent["interfaces"], json!(["ledger", "mail"]), "the owner's capability survives: {agent}");
    let fields = agent["inputFields"].as_array().expect("questions");
    let by_key = |k: &str| fields.iter().find(|f| f["key"] == k).cloned();
    assert!(by_key("po_prefix").is_some(), "the owner's question survives: {fields:?}");
    // Both package changes arrive.
    assert_eq!(by_key("invoice_mailbox").unwrap()["label"], "Which mailbox do bills arrive in?", "the corrected label: {fields:?}");
    assert!(by_key("approver").is_some(), "the new question: {fields:?}");
    assert_eq!(fields.len(), 3);
    // The one reader reads the merged declaration the same way.
    let cfg = napp::agent::parse_agent_config(&nebo.agent(ID).frontmatter).unwrap();
    assert_eq!(cfg.requires.interfaces, vec!["ledger", "mail"]);
    assert_eq!(cfg.inputs.iter().map(|i| i.key.as_str()).collect::<Vec<_>>(), vec!["invoice_mailbox", "approver", "po_prefix"]);
}

/// The owner sets a department: it is locked, the roster shows it, and the
/// write a package sync makes (`set_agent_department`, the one call
/// `persist_agent_from_api` makes for a package's department) does not move
/// it. A reporting line A→B→C stands; C→A is refused with the loop named;
/// a seat cannot report to itself; the roster carries the line.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn department_locks_and_reporting_line_refuses_cycles() {
    let nebo = session().await;
    let a = nebo.hire("Alder", json!({ "workflows": {} })).await;
    let b = nebo.hire("Birch", json!({ "workflows": {} })).await;
    let c = nebo.hire("Cedar", json!({ "workflows": {} })).await;
    assert_eq!(nebo.agent(&a).department_locked, 0);

    // The package's department, as an install writes it: unlocked, movable.
    nebo.store().set_agent_department(&a, Some("sales")).unwrap();
    assert_eq!(nebo.agent(&a).department.as_deref(), Some("sales"));
    // The owner's department: locked.
    nebo.put_ok(&format!("/agents/{a}"), &json!({ "department": "Revenue" })).await;
    let row = nebo.agent(&a);
    assert_eq!(row.department.as_deref(), Some("Revenue"));
    assert_eq!(row.department_locked, 1);
    // A later package sync does not move it.
    nebo.store().set_agent_department(&a, Some("sales")).unwrap();
    assert_eq!(nebo.agent(&a).department.as_deref(), Some("Revenue"), "the owner's department outlives the sync");
    let roster = nebo.get_ok("/agents").await;
    let alder = roster["agents"].as_array().unwrap().iter().find(|x| x["id"] == a).unwrap();
    assert_eq!(alder["department"], "Revenue", "{alder}");
    // The other seats are untouched by any of it.
    assert_eq!(nebo.agent(&b).department_locked, 0);

    // A→B→C stands.
    nebo.put_ok(&format!("/agents/{a}"), &json!({ "reportsTo": b })).await;
    nebo.put_ok(&format!("/agents/{b}"), &json!({ "reportsTo": c })).await;
    assert_eq!(
        nebo.store().manager_chain(&a).unwrap(),
        vec![(b.clone(), "Birch".to_string()), (c.clone(), "Cedar".to_string())]
    );
    // C→A closes the loop: refused, and the loop is named.
    let (status, body) = nebo.put(&format!("/agents/{c}"), &json!({ "reportsTo": a })).await;
    assert_eq!(status, 400, "{body}");
    let message = body.to_string();
    assert!(message.contains("Cedar cannot report to Alder: that closes a loop"), "{message}");
    assert!(message.contains("Alder answers to Birch answers to Cedar"), "{message}");
    assert!(nebo.agent(&c).reports_to.is_none(), "nothing was written");
    // A seat cannot report to itself; an unknown manager is refused.
    let (status, body) = nebo.put(&format!("/agents/{c}"), &json!({ "reportsTo": c })).await;
    assert_eq!(status, 400);
    assert!(body.to_string().contains("cannot report to itself"), "{body}");
    let (status, _) = nebo.put(&format!("/agents/{c}"), &json!({ "reportsTo": "no-such-seat" })).await;
    assert_eq!(status, 400);
    // The roster carries the line; a blank clears it.
    let roster = nebo.get_ok("/agents").await;
    let alder = roster["agents"].as_array().unwrap().iter().find(|x| x["id"] == a).unwrap();
    assert_eq!(alder["reportsTo"], b, "{alder}");
    nebo.put_ok(&format!("/agents/{a}"), &json!({ "reportsTo": "" })).await;
    assert!(nebo.agent(&a).reports_to.is_none(), "blank = answers to the owner");
    assert_eq!(nebo.agent(&a).department.as_deref(), Some("Revenue"), "the department was not touched by a line edit");
}

/// Every door an employee is hired through grants the job its package
/// declares, through the one grant (`codes::hire`): a code the owner pastes
/// in their chat, the Hire tap, a hire on the owner's account (the hub's
/// install event), the hire card (`POST /codes`), the `hire_employee` tool
/// in the owner's own run, a collection, and the owner's create from a
/// package. A code posted in a chat channel, or the tool in a run that
/// isn't the owner's, hires with no job: only the owner consents.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_hiring_door_grants_the_declared_job() {
    use agent::ChannelDispatcher;
    use types::permissions::{Effect, RuleKey, RuleSource, Scope};

    let nebo = session().await;
    let profile = uuid::Uuid::new_v4().to_string();
    nebo.store()
        .create_auth_profile(
            &profile,
            "NeboAI",
            "neboai",
            "proof-token",
            None,
            None,
            0,
            1,
            Some("token"),
            None,
        )
        .unwrap();
    let job = json!({ "requires": { "interfaces": ["mail", "calendar"] }, "workflows": {} });
    // Each door hires its own employee.
    let offer = |n: u32, name: &str| {
        let code = format!("AGNT-HYRE-{n:04}");
        let id = format!("hire-door-{n}");
        hub_offers_agent(&code, &id, name, job.clone());
        (code, id)
    };
    // The job the owner's consent granted: its capability allow rules.
    let granted = |id: &str| -> Vec<String> {
        let mut caps: Vec<String> = nebo
            .store()
            .permission_rules_in(&Scope::Employee(id.to_string()))
            .unwrap()
            .into_iter()
            .filter(|r| r.effect == Effect::Allow && matches!(r.source, RuleSource::Hire { .. }))
            .filter_map(|r| match r.key {
                RuleKey::Capability(c) => Some(c),
                _ => None,
            })
            .collect();
        caps.sort();
        caps
    };
    let the_job = vec!["calendar".to_string(), "mail".to_string()];
    let mut hired = Vec::new();

    // A code the owner pastes in their own chat (the WS door's call).
    let (code, pasted) = offer(1, "Pasted Hire");
    crate::codes::handle_code(
        &nebo.state,
        crate::codes::CodeType::Agent,
        &code,
        &crate::handlers::ws::EventOrigin::unclaimed("agent:main:web"),
    )
    .await;
    assert_eq!(granted(&pasted), the_job, "pasted code");
    hired.push(pasted);

    // The Hire tap on the store.
    let (_, tapped) = offer(2, "Tapped Hire");
    nebo.post_ok(&format!("/store/products/{tapped}/install"), &json!({}))
        .await;
    assert_eq!(granted(&tapped), the_job, "Hire tap");
    hired.push(tapped);

    // A hire on the owner's account, from the web or the phone: the hub's
    // install event.
    let (_, remote) = offer(3, "Remote Hire");
    let event = napp::InstallEvent {
        event_type: "tool_installed".into(),
        tool_id: remote.clone(),
        payload: json!({}),
    };
    crate::handle_comm_install_event(&nebo.state, event)
        .await
        .expect("the install event");
    assert_eq!(granted(&remote), the_job, "hub hire");
    hired.push(remote);

    // The hire card in the owner's chat redeems through `POST /codes`.
    let (code, carded) = offer(4, "Card Hire");
    nebo.post_ok("/codes", &json!({ "code": code })).await;
    assert_eq!(granted(&carded), the_job, "hire card");
    hired.push(carded);

    // The `hire_employee` tool, called in the owner's own run.
    let (code, tooled) = offer(5, "Tool Hire");
    let owner_run = Nebo::ctx("", Origin::User);
    let r = nebo
        .tool(&owner_run, "hire_employee", json!({ "code": code }))
        .await;
    assert!(!r.is_error, "{}", r.content);
    assert_eq!(
        granted(&tooled),
        the_job,
        "hire_employee in the owner's run"
    );
    hired.push(tooled);

    // A collection: each employee in it is hired by the owner's act.
    let (item, collected) = offer(6, "Collected Hire");
    hub_offers_collection(
        "COLL-HYRE-0001",
        "hire-collection",
        "Hire Pack",
        &[item.as_str()],
    );
    nebo.post_ok("/codes", &json!({ "code": "COLL-HYRE-0001" }))
        .await;
    assert_eq!(granted(&collected), the_job, "collection");
    hired.push(collected);

    // The owner's own create from a package.
    let created = nebo.hire("Created Hire", job.clone()).await;
    assert_eq!(granted(&created), the_job, "owner's create");
    hired.push(created);

    // Not the owner's act: a code posted in a chat channel.
    let (code, channelled) = offer(7, "Channel Hire");
    let channel = crate::channel_dispatch::ChannelDispatchImpl::new(nebo.state.clone());
    let reply = channel
        .dispatch(
            "",
            "slack:dm:stranger",
            tools::ChannelContext::default(),
            &code,
        )
        .await
        .expect("the channel's reply");
    assert!(
        nebo.store().get_agent(&channelled).unwrap().is_some(),
        "installed: {reply:?}"
    );
    assert!(
        granted(&channelled).is_empty(),
        "a channel's code grants no job"
    );
    hired.push(channelled);

    // Not the owner's act: the tool in a run that came in from a channel.
    let (code, unattended) = offer(8, "Unattended Hire");
    let r = nebo
        .tool(
            &Nebo::ctx("", Origin::Comm),
            "hire_employee",
            json!({ "code": code }),
        )
        .await;
    assert!(!r.is_error, "{}", r.content);
    assert!(
        nebo.store().get_agent(&unattended).unwrap().is_some(),
        "installed: {}",
        r.content
    );
    assert!(
        granted(&unattended).is_empty(),
        "a hire outside the owner's run grants no job"
    );
    hired.push(unattended);

    // A rehire never undoes what the owner set: a capability the owner
    // turned off stays off.
    let (code, rehired) = offer(9, "Rehired Hire");
    crate::codes::handle_code(
        &nebo.state,
        crate::codes::CodeType::Agent,
        &code,
        &crate::handlers::ws::EventOrigin::unclaimed("agent:main:web"),
    )
    .await;
    nebo.put_ok(
        &format!("/entity-config/agent/{rehired}"),
        &json!({ "permissions": { "mail": false } }),
    )
    .await;
    crate::codes::handle_code(
        &nebo.state,
        crate::codes::CodeType::Agent,
        &code,
        &crate::handlers::ws::EventOrigin::unclaimed("agent:main:web"),
    )
    .await;
    let mail = nebo
        .store()
        .permission_rules_in(&Scope::Employee(rehired.clone()))
        .unwrap()
        .into_iter()
        .find(|r| r.key == RuleKey::Capability("mail".into()) && r.field.is_none())
        .expect("the owner's rule");
    assert_eq!(mail.effect, Effect::Deny, "the owner's setting stands");
    hired.push(rehired);

    // Leave the shared server as it was found.
    for id in hired {
        let _ = nebo.delete(&format!("/agents/{id}")).await;
    }
    nebo.store().delete_auth_profile(&profile).unwrap();
}

/// This computer's host for a scenario, under its own folders, able to hire
/// Claude Code as the fake coding agent (`company_agent::fake_company_agent`,
/// writing what it is told to `told`), hiring none yet: what "Hire from
/// another app" reaches on this computer.
fn claude_code_here(bot: &'static str, root: &Path, told: &Path) -> Arc<ai::LocalHost> {
    let host = ai::LocalHost::open(
        Arc::new(move || Some(bot.to_owned())),
        root.join("link"),
        root.join("home"),
        Some(root.join("nebo-link")),
    )
    .unwrap();
    host.tell(
        nebo_runtimes::acp::Agent::ClaudeCode,
        nebo_runtimes::RuntimeCommand {
            program: std::env::current_exe().unwrap().to_string_lossy().into_owned(),
            args: ["staffed_proof::company_agent::fake_company_agent", "--exact", "--nocapture", "--test-threads=1"]
                .map(String::from)
                .to_vec(),
            env: vec![("NEBO_PROOF_COMPANY".into(), told.to_string_lossy().into_owned())],
        },
    );
    host
}

/// The hire list's entries of `runtime` on one computer as (id, name,
/// hired). Only the runtime asked about: what else this machine has
/// installed is listed too.
fn hire_entries(listing: &Value, computer: &str, runtime: &str) -> Vec<(String, String, bool)> {
    listing["computers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == computer)
        .unwrap_or_else(|| panic!("{computer} is not listed: {listing}"))["agents"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["runtime"] == runtime)
        .map(|a| (a["id"].as_str().unwrap().to_owned(), a["name"].as_str().unwrap().to_owned(), a["hired"] == true))
        .collect()
}

/// The owner taps Claude Code twice in "Hire from another app" and names
/// each: two employees, each its own Claude Code in a folder of its own,
/// and each conversation a session of its own agent in its own folder. A
/// third named like one of them is refused in the owner's words before
/// anything is started. The hire list still offers Claude Code once, to
/// start new, with both on the team by the names he gave them. Through the
/// create-agent handler, this computer's real host and a coding agent
/// process; no network.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_claude_code_hire_is_an_employee_of_its_own() {
    const BOT: &str = "5e1f0000-0000-4000-8000-00000000c0d2";
    let nebo = session().await;
    let root = tempfile::tempdir().unwrap();
    let told = root.path().join("told.jsonl");
    let host = claude_code_here(BOT, root.path(), &told);
    let mut state = nebo.state.clone();
    state.local_host = Some(host.clone());
    state.linked_apps = Arc::new(Default::default());
    let hire = |name: &str| {
        let state = state.clone();
        let body = json!({ "linked": { "botId": BOT, "agentId": "new:claude-code" }, "name": name });
        async move {
            crate::handlers::agents::create_agent(axum::extract::State(state), axum::http::HeaderMap::new(), axum::Json(body)).await
        }
    };

    let front = hire("Proof Frontend").await.expect("the first is hired").0;
    let back = hire("Proof Backend").await.expect("the second is hired").0;
    let (front, back) = (front["agent"]["id"].as_str().unwrap().to_owned(), back["agent"]["id"].as_str().unwrap().to_owned());
    assert_ne!(front, back);
    assert_eq!((nebo.agent(&front).name, nebo.agent(&back).name), ("Proof Frontend".to_owned(), "Proof Backend".to_owned()));

    // Two coding agents on this computer, each in a folder of its own, each
    // the brain of one employee.
    let hosted = host.agents();
    assert_eq!(hosted.len(), 2, "one agent per hire");
    assert_ne!(hosted[0].id, hosted[1].id);
    assert_ne!(hosted[0].acp.workdir, hosted[1].acp.workdir);
    let own = root.path().join("home").join("NeboAI").canonicalize().unwrap();
    assert!(hosted.iter().all(|a| a.acp.workdir.is_dir() && a.acp.workdir.starts_with(&own)), "{hosted:?}");
    let brain = |id: &str| nebo.store().get_entity_config("agent", id).unwrap().and_then(|c| c.model_preference).unwrap();
    let agent_of = |id: &str| brain(id).rsplit('/').next().unwrap().to_owned();
    let (front_agent, back_agent) = (agent_of(&front), agent_of(&back));
    assert_ne!(front_agent, back_agent);
    let folder = |agent: &str| hosted.iter().find(|a| a.id == agent).unwrap().acp.workdir.clone();

    // A name already on the team is refused, whatever its case, and nothing
    // was started for it.
    let (status, refused) = hire("proof frontend").await.expect_err("a taken name");
    assert_eq!(status.as_u16(), 400);
    assert_eq!(refused.0.error, "You already have an employee named \"Proof Frontend\". Pick another name.");
    assert_eq!(host.agents().len(), 2, "nothing started for a refused name");
    assert_eq!(std::fs::read_dir(&own).unwrap().count(), 2, "no folder made for a refused name");

    // Each employee's conversation is a session of its own agent, opened in
    // its own folder.
    let provider = ai::LinkedProvider::new(
        ai::Relay::Hub { api_url: "http://127.0.0.1:9".into(), token: Arc::new(|| None) },
        nebo.store().clone(),
        Some(host.clone()),
        "Nebo proof",
    );
    for (employee, agent) in [(&front, &front_agent), (&back, &back_agent)] {
        let chat = uuid::Uuid::new_v4().to_string();
        nebo.store()
            .create_chat_for_session(&chat, &format!("agent:{employee}:thread:{chat}"), "Proof", None)
            .unwrap();
        let req = ai::ChatRequest {
            messages: vec![ai::Message { role: "user".into(), content: "How many files are left?".into(), ..Default::default() }],
            model: format!("{BOT}/{agent}"),
            chat_id: chat,
            ..ai::ChatRequest::new(ai::RequestTrace { agent_id: employee.clone(), ..ai::RequestTrace::new("agent_turn") })
        };
        let mut rx = ai::Provider::stream(&provider, &req).await.unwrap();
        let mut said = String::new();
        while let Some(e) = tokio::time::timeout(Duration::from_secs(60), rx.recv()).await.expect("the turn ended") {
            said.push_str(&e.text);
        }
        assert!(said.contains("ANSWER: 3 files left."), "{said}");
    }
    let opened: Vec<PathBuf> = std::fs::read_to_string(&told)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|t| t.get("new").is_some())
        .map(|t| PathBuf::from(t["cwd"].as_str().unwrap()))
        .collect();
    assert_eq!(opened, [folder(&front_agent), folder(&back_agent)], "one session per employee, each in its own folder");

    // The hire list: Claude Code once, to start new; both on the team.
    let listing = crate::handlers::agents::list_linked_agents(axum::extract::State(state.clone())).await.unwrap().0;
    assert_eq!(
        hire_entries(&listing, "This computer", "claude-code"),
        [
            (front_agent.clone(), "Proof Frontend".to_owned(), true),
            (back_agent.clone(), "Proof Backend".to_owned(), true),
            ("new:claude-code".to_owned(), "Claude Code".to_owned(), false),
        ]
    );

    host.shutdown(Duration::from_secs(5)).await;
    for id in [&front, &back] {
        let _ = nebo.delete(&format!("/agents/{id}")).await;
    }
}

/// The hire list is known before it is asked: a listing answers at once
/// with what was last found on the owner's computers and looks again behind
/// it; an app found meanwhile reaches every open list in ONE
/// `linked_apps_changed` event, and the next listing has it. Only the very
/// first listing, before anything was found, waits for the look.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_hire_list_answers_from_what_is_known_and_an_app_found_later_is_sent() {
    const BOT: &str = "5e1f0000-0000-4000-8000-00000000c0d3";
    let nebo = session().await;
    let root = tempfile::tempdir().unwrap();
    let host = claude_code_here(BOT, root.path(), &root.path().join("told.jsonl"));
    let mut state = nebo.state.clone();
    state.local_host = Some(host.clone());
    state.linked_apps = Arc::new(Default::default());
    let list = || crate::handlers::agents::list_linked_agents(axum::extract::State(state.clone()));
    // Any other coding agent that speaks ACP: never one this machine has
    // installed itself, so it is there only once it is told.
    let other = |listing: &Value| !hire_entries(listing, "This computer", "acp").is_empty();
    let mut events = nebo.state.hub.subscribe();

    // The first listing waits for the first look.
    let first = list().await.unwrap().0;
    assert!(first["checkedAt"].as_i64().is_some_and(|t| t > 0), "{first}");
    assert_eq!(hire_entries(&first, "This computer", "claude-code"), [("new:claude-code".to_owned(), "Claude Code".to_owned(), false)]);
    assert!(!other(&first), "{first}");

    // Another coding agent is installed meanwhile: the next listing answers
    // with what was known, and the look behind it sends the list with it.
    host.tell(
        nebo_runtimes::acp::Agent::Other,
        nebo_runtimes::RuntimeCommand { program: "proof-acp".into(), args: Vec::new(), env: Vec::new() },
    );
    let known = list().await.unwrap().0;
    assert_eq!(known["computers"], first["computers"], "answered from what was known, not after a look");
    let sent = loop {
        let e = tokio::time::timeout(Duration::from_secs(30), events.recv()).await.expect("the change is sent").unwrap();
        if e.event_type == "linked_apps_changed" {
            break e.payload;
        }
    };
    assert_eq!(hire_entries(&sent, "This computer", "acp"), [("new:acp".to_owned(), "ACP agent".to_owned(), false)], "{sent}");
    let next = list().await.unwrap().0;
    assert_eq!(next["computers"], sent["computers"]);
}
