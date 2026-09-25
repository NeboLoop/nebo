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
