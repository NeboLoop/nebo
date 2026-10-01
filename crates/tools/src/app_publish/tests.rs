use std::sync::Mutex;

use super::*;

fn temp_store() -> (tempfile::TempDir, Arc<db::Store>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(db::Store::new(dir.path().join("t.db").to_str().unwrap()).unwrap());
    (dir, store)
}

fn set_mode(store: &db::Store, on: bool) {
    store
        .update_settings(
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(on),
        )
        .unwrap();
}

/// An app employee with a package folder (manifest, persona, config) and a
/// page.
fn seed_app(store: &db::Store, root: &Path, id: &str, name: &str) -> PathBuf {
    let pkg = root.join(name);
    std::fs::create_dir_all(pkg.join("ui/assets")).unwrap();
    std::fs::write(
        pkg.join("manifest.json"),
        r#"{"id":"local","name":"kart","version":"1.0.0","type":"app","artifact_type":"app",
            "description":"Race karts around three tracks against the clock.",
            "permissions":["storage:readwrite"],
            "window":{"title":"Kart Racer","width":420,"height":800,"resizable":false}}"#,
    )
    .unwrap();
    std::fs::write(pkg.join("AGENT.md"), "---\nname: kart\ndescription: Builds the kart game\n---\n# Kart Racer\n\nA quick arcade racer.\n\nBeat your best lap on three tracks.\n").unwrap();
    std::fs::write(pkg.join("agent.json"), r#"{"workflows":{}}"#).unwrap();
    std::fs::write(
        pkg.join("ui/index.html"),
        "<html><head><title>Kart</title></head><body>go</body></html>",
    )
    .unwrap();
    std::fs::write(pkg.join("ui/assets/main.js"), "start()").unwrap();
    std::fs::write(pkg.join("ui/.DS_Store"), "x").unwrap();
    store
        .create_agent(id, None, name, "A kart racing game", "", "", None, None)
        .unwrap();
    store
        .set_agent_app_fields(id, true, Some(pkg.join("ui").to_str().unwrap()), None, None)
        .unwrap();
    store
        .set_agent_napp_path(id, pkg.to_str().unwrap())
        .unwrap();
    pkg
}

fn manifest() -> Value {
    json!({
        "version": "1.0.0",
        "description": "Race karts around three tracks against the clock.",
        "permissions": ["storage:readwrite"],
        "tags": ["game"],
        "window": {"title": "Kart Racer", "width": 420, "height": 800}
    })
}

// ── The draft ───────────────────────────────────────────────────────

/// The listing is drafted from the app itself: the window's title, the
/// manifest's description and permissions and version, the persona's prose,
/// and a category the app's words name.
#[test]
fn the_draft_comes_from_the_manifest_and_the_code() {
    let m = manifest();
    let cats = vec!["Productivity".to_string(), "Games".to_string()];
    let d = build_draft(&DraftSource {
        display_name: "Kart Racing Game Developer",
        description: "",
        manifest: &m,
        agent_md: "---\nname: kart\n---\n# Kart\n\nA quick arcade racer.\n\n- three tracks\n",
        index_html: None,
        previous: None,
        published_version: None,
        categories: &cats,
    });
    assert_eq!(d.name, "Kart Racer");
    assert_eq!(
        d.short_description,
        "Race karts around three tracks against the clock."
    );
    assert!(
        d.long_description.contains("A quick arcade racer."),
        "{}",
        d.long_description
    );
    assert_eq!(d.version, "1.0.0");
    assert_eq!(d.visibility, "public");
    assert_eq!(d.permissions, vec!["storage:readwrite"]);
    assert_eq!(d.category.as_deref(), Some("Games"));
    assert_eq!(d.window.as_ref().unwrap()["width"], 420);
}

/// With nothing in the manifest, the page's own title and description and
/// the employee's name fill in; a long description is capped at 500.
#[test]
fn the_page_and_the_employee_fill_what_the_manifest_lacks() {
    let empty = json!({});
    let long = "word ".repeat(200);
    let html = format!(
        "<html><head><title>Tide Clock</title><meta name=\"description\" content=\"{long}\"></head></html>"
    );
    let d = build_draft(&DraftSource {
        display_name: "",
        description: "",
        manifest: &empty,
        agent_md: "",
        index_html: Some(&html),
        previous: None,
        published_version: None,
        categories: &[],
    });
    assert_eq!(d.name, "Tide Clock");
    assert!(d.short_description.chars().count() <= 500 && d.short_description.ends_with('…'));
    assert_eq!(d.version, "1.0.0");
    assert_eq!(d.category, None);
}

/// What the owner shaped stays across a redraft; a version already sent for
/// review moves on a patch, and a higher one the owner chose stands.
#[test]
fn a_redraft_keeps_the_owners_edits_and_moves_past_the_published_version() {
    let m = manifest();
    let prev = ListingDraft {
        name: "Kart Rush".into(),
        short_description: "Fast laps.".into(),
        long_description: "Owner's words.".into(),
        category: Some("Games".into()),
        version: "1.0.0".into(),
        visibility: "unlisted".into(),
        screenshots: vec![ListingShot {
            file_id: "f1".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let src = |published| DraftSource {
        display_name: "x",
        description: "",
        manifest: &m,
        agent_md: "",
        index_html: None,
        previous: Some(&prev),
        published_version: published,
        categories: &[],
    };
    let d = build_draft(&src(Some("1.0.0")));
    assert_eq!(
        (
            d.name.as_str(),
            d.short_description.as_str(),
            d.visibility.as_str()
        ),
        ("Kart Rush", "Fast laps.", "unlisted")
    );
    assert_eq!(d.version, "1.0.1");
    assert_eq!(d.screenshots.len(), 1);
    let mut higher = prev.clone();
    higher.version = "2.0.0".into();
    let d = build_draft(&DraftSource {
        previous: Some(&higher),
        ..src(Some("1.0.0"))
    });
    assert_eq!(d.version, "2.0.0");
}

/// The owner's changes land; screenshots are kept and reordered by id, and
/// an id that is not the listing's, a bad version or visibility is refused.
#[test]
fn edits_shape_the_listing_and_bad_ones_are_refused() {
    let mut d = ListingDraft {
        name: "Kart".into(),
        short_description: "Race karts.".into(),
        version: "1.0.0".into(),
        visibility: "public".into(),
        screenshots: ["a", "b", "c"]
            .iter()
            .map(|id| ListingShot {
                file_id: id.to_string(),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    apply_edits(&mut d, &json!({"short_description": "  Race   karts\nfast. ", "screenshots": ["b", "a"], "visibility": "Unlisted"})).unwrap();
    assert_eq!(d.short_description, "Race karts fast.");
    assert_eq!(
        d.screenshots
            .iter()
            .map(|s| s.file_id.as_str())
            .collect::<Vec<_>>(),
        vec!["b", "a"]
    );
    assert_eq!(d.visibility, "unlisted");
    assert!(apply_edits(&mut d, &json!({"screenshots": ["zzz"]})).is_err());
    assert!(apply_edits(&mut d, &json!({"version": "v2"})).is_err());
    assert!(apply_edits(&mut d, &json!({"visibility": "everyone"})).is_err());
}

#[test]
fn a_listing_without_screenshots_or_a_real_description_is_not_ready() {
    let d = ListingDraft {
        name: "Kart".into(),
        short_description: "Race".into(),
        version: "1.0.0".into(),
        visibility: "public".into(),
        ..Default::default()
    };
    let p = problems(&d, &["Games".to_string()]);
    assert!(p.iter().any(|x| x.contains("10 characters")));
    assert!(p.iter().any(|x| x.contains("screenshots")));
    assert!(p.iter().any(|x| x.contains("category")));
    let ready = ListingDraft {
        short_description: "Race karts around three tracks.".into(),
        category: Some("Games".into()),
        screenshots: vec![ListingShot {
            file_id: "f".into(),
            ..Default::default()
        }],
        ..d
    };
    assert!(problems(&ready, &["Games".to_string()]).is_empty());
}

#[test]
fn the_persona_is_typed_as_an_app_for_the_hub() {
    assert_eq!(
        with_app_frontmatter("# Hi"),
        "---\nartifact_type: app\n---\n# Hi"
    );
    let typed = with_app_frontmatter("---\nname: kart\n---\n# Kart\n");
    assert!(
        typed.starts_with("---\nname: kart\nartifact_type: app\n---\n# Kart"),
        "{typed}"
    );
    let already = "---\nartifact_type: app\n---\nbody";
    assert_eq!(with_app_frontmatter(already), already);
}

// ── The bundle ──────────────────────────────────────────────────────

#[test]
fn the_bundle_carries_the_package_and_every_page_file() {
    let (dir, store) = temp_store();
    let pkg = seed_app(&store, dir.path(), "app-1", "Kart Racer");
    let (zip, pages) = build_bundle("# Kart", Some(&pkg), &pkg.join("ui"), None).unwrap();
    assert_eq!(pages, 2);
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip)).unwrap();
    let mut names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "AGENT.md",
            "agent.json",
            "manifest.json",
            "ui/assets/main.js",
            "ui/index.html"
        ]
    );
    let mut md = String::new();
    std::io::Read::read_to_string(&mut archive.by_name("AGENT.md").unwrap(), &mut md).unwrap();
    assert!(md.contains("artifact_type: app"));

    let empty = dir.path().join("empty-ui");
    std::fs::create_dir_all(&empty).unwrap();
    assert!(
        build_bundle("# x", None, &empty, None)
            .unwrap_err()
            .contains("empty")
    );
    let big = dir.path().join("big-ui");
    std::fs::create_dir_all(&big).unwrap();
    std::fs::write(
        big.join("video.mp4"),
        vec![0u8; (MAX_BUNDLE_FILE + 1) as usize],
    )
    .unwrap();
    assert!(
        build_bundle("# x", None, &big, None)
            .unwrap_err()
            .contains("at most 10 MB")
    );
}

/// The bundle carries the employee's own skills under `skills/<name>/`: the
/// package's skill folders and the plain-named skills its agent.json lists
/// from the bot's skills folder, by the hub's rules: its SKILL.md and the
/// types a skill may carry, scripts of any type, no dot files, build
/// folders or reserved names, no marketplace references, and the same
/// per-file limit as every bundle file.
#[test]
fn the_bundle_carries_the_employees_own_skills() {
    let (dir, store) = temp_store();
    let pkg = seed_app(&store, dir.path(), "app-1", "Kart Racer");
    let put = |p: PathBuf, body: &[u8]| {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    };
    let tune = pkg.join("skills/tune-karts");
    put(tune.join("SKILL.md"), b"---\nname: tune-karts\n---\nTune.");
    put(tune.join("references/tracks.md"), b"three tracks");
    put(tune.join("scripts/lap-timer"), b"#!/bin/sh\necho lap");
    put(tune.join("assets/logo.png"), b"png");
    put(tune.join("notes.exe"), b"no");
    put(tune.join(".DS_Store"), b"no");
    put(tune.join("node_modules/x/index.js"), b"no");
    put(tune.join("dist/out.js"), b"no");
    put(tune.join("agent.json"), b"{}");
    // A folder without a SKILL.md is not a skill.
    put(pkg.join("skills/scratch/notes.md"), b"no");

    let user_skills = dir.path().join("user-skills");
    put(user_skills.join("score-board/SKILL.md"), b"---\nname: score-board\n---\nScores.");
    put(user_skills.join("score-board/template.html"), b"<table>");
    put(user_skills.join("not-mine/SKILL.md"), b"---\nname: not-mine\n---\n");
    std::fs::write(
        pkg.join("agent.json"),
        r#"{"skills": ["score-board", "tune-karts", "@acme/skills/web-search@^1.0.0", "../escape", "missing"]}"#,
    )
    .unwrap();

    let (zip, _) = build_bundle("# Kart", Some(&pkg), &pkg.join("ui"), Some(&user_skills)).unwrap();
    let archive = zip::ZipArchive::new(std::io::Cursor::new(zip)).unwrap();
    let mut skills: Vec<String> = archive
        .file_names()
        .filter(|n| n.starts_with("skills/"))
        .map(String::from)
        .collect();
    skills.sort();
    assert_eq!(
        skills,
        vec![
            "skills/score-board/SKILL.md",
            "skills/score-board/template.html",
            "skills/tune-karts/SKILL.md",
            "skills/tune-karts/assets/logo.png",
            "skills/tune-karts/references/tracks.md",
            "skills/tune-karts/scripts/lap-timer",
        ]
    );
    assert!(archive.file_names().any(|n| n == "ui/index.html"), "the page still ships");

    // A skill file past the hub's limit is refused in words, like any file.
    put(tune.join("references/huge.md"), &vec![b'x'; (MAX_BUNDLE_FILE + 1) as usize]);
    let err = build_bundle("# Kart", Some(&pkg), &pkg.join("ui"), Some(&user_skills)).unwrap_err();
    assert!(err.contains("skills/tune-karts/references/huge.md") && err.contains("at most 10 MB"), "{err}");
}

// ── Who gets the pack ───────────────────────────────────────────────

/// The publish tools are in the one developer pack: withheld with the mode
/// off, offered to the app and the employees that work with it with the
/// mode on, and never to anyone else.
#[test]
fn the_publish_tools_are_in_the_one_developer_pack() {
    let (dir, store) = temp_store();
    seed_app(&store, dir.path(), "app-1", "Kart Racer");
    store
        .create_agent("dev-1", None, "Game Developer", "", "", "", None, None)
        .unwrap();
    store
        .create_agent("acct-1", None, "Bookkeeper", "", "", "", None, None)
        .unwrap();
    store
        .update_agent(
            "dev-1",
            "",
            "",
            "",
            "",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("app-1"),
        )
        .unwrap();
    let ours = [APP_SCREENSHOT, APP_LISTING, APP_SUBMIT];
    // Mode off: the owner's own app builds and publishes itself.
    let withheld = crate::app_dev::withheld(&store, "app-1");
    assert!(
        ours.iter().all(|t| !withheld.iter().any(|w| w == t)),
        "mode off: the app itself gets them"
    );
    for who in ["dev-1", "acct-1"] {
        let withheld = crate::app_dev::withheld(&store, who);
        assert!(
            ours.iter().all(|t| withheld.iter().any(|w| w == t)),
            "mode off: {who} gets none of them"
        );
    }
    // An app installed from the marketplace never does, mode or not.
    seed_app(&store, dir.path(), "app-9", "Bought Game");
    store.set_agent_napp_path("app-9", "/data/nebo/agents/bought-game.napp").unwrap();
    for on in [false, true] {
        set_mode(&store, on);
        let withheld = crate::app_dev::withheld(&store, "app-9");
        assert!(
            ours.iter().all(|t| withheld.iter().any(|w| w == t)),
            "an installed app is never built here (mode {on})"
        );
    }
    set_mode(&store, false);
    set_mode(&store, true);
    for who in ["app-1", "dev-1"] {
        let withheld = crate::app_dev::withheld(&store, who);
        assert!(
            ours.iter().all(|t| !withheld.iter().any(|w| w == t)),
            "mode on: {who} gets them"
        );
    }
    let withheld = crate::app_dev::withheld(&store, "acct-1");
    assert!(
        ours.iter().all(|t| withheld.iter().any(|w| w == t)),
        "an unrelated employee never does"
    );
}

// ── Submitting needs the owner's yes ────────────────────────────────

#[derive(Default)]
struct FakeHub {
    calls: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl PublishHub for FakeHub {
    async fn categories(&self) -> Vec<String> {
        vec!["Games".into()]
    }
    async fn agent(&self, args: Value) -> Result<Value, String> {
        let action = args["action"].as_str().unwrap_or("").to_string();
        self.calls.lock().unwrap().push(format!("agent:{action}"));
        Ok(match action.as_str() {
            "create" => {
                assert!(
                    args["manifestContent"]
                        .as_str()
                        .unwrap()
                        .contains("artifact_type: app")
                );
                json!({"id": "art-1"})
            }
            "bundle-token" => json!({"token": "upload-token"}),
            "submit" => json!({"status": "manual_review", "message": "In review."}),
            "update" => {
                if args.get("screenshots").is_some() {
                    assert_eq!(args["screenshots"], json!(["shot-1"]));
                    assert_eq!(args["visibility"], "public");
                }
                json!({"updated": true})
            }
            _ => json!({}),
        })
    }
    async fn upload_bundle(&self, id: &str, token: &str, zip: Vec<u8>) -> Result<Value, String> {
        assert_eq!((id, token), ("art-1", "upload-token"));
        assert!(!zip.is_empty());
        self.calls.lock().unwrap().push("upload_bundle".into());
        Ok(json!({"uiFilesStored": 2}))
    }
    async fn upload_file(&self, _f: &str, _m: &str, _d: Vec<u8>) -> Result<String, String> {
        Ok("file".into())
    }
}

/// A ready listing for app-1 on a store with the mode on.
fn ready_listing(store: &db::Store) {
    let draft = ListingDraft {
        name: "Kart Racer".into(),
        short_description: "Race karts around three tracks.".into(),
        long_description: "Beat your best lap.".into(),
        category: Some("Games".into()),
        version: "1.0.0".into(),
        visibility: "public".into(),
        screenshots: vec![ListingShot {
            file_id: "shot-1".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    store
        .put_app_listing(&db::AppListing {
            app_id: "app-1".into(),
            draft: serde_json::to_string(&draft).unwrap(),
            status: "draft".into(),
            ..Default::default()
        })
        .unwrap();
}

/// A conversation the owner is in, whose ask card he answers with `answer`.
fn owner_answers(answer: &'static str) -> ToolContext {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<ai::StreamEvent>(8);
    let channels: crate::origin::AskChannels = Default::default();
    let ch = channels.clone();
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if ev.event_type == ai::StreamEventType::AskRequest {
                let id = ev.error.clone().unwrap_or_default();
                // The card waits for the tap; it comes a moment later.
                for _ in 0..50 {
                    if let Some(reply) = ch.lock().await.remove(&id) {
                        let _ = reply.send(answer.to_string());
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
    });
    ToolContext {
        origin: crate::origin::Origin::User,
        session_key: "agent:app-1:web".into(),
        stream_tx: Some(tx),
        ask_channels: Some(channels),
        ..Default::default()
    }
}

/// Nothing is submitted without the owner's yes on the card: an unattended
/// run can't, a "not yet" sends nothing, and only "Submit for review" walks
/// the publish path: create, bundle token, bundle, listing, submit.
#[tokio::test]
async fn submit_is_impossible_without_the_owners_explicit_yes() {
    let (dir, store) = temp_store();
    set_mode(&store, true);
    seed_app(&store, dir.path(), "app-1", "Kart Racer");
    ready_listing(&store);
    let hub = Arc::new(FakeHub::default());
    let tool = AppSubmitTool(Arc::new(
        Publisher::new(store.clone()).with_hub(hub.clone()),
    ));
    let submit = json!({});

    // Unattended: nobody can say yes.
    let unattended = ToolContext {
        origin: crate::origin::Origin::Workflow,
        session_key: "agent:app-1:web".into(),
        ..Default::default()
    };
    let r = tool.execute_dyn(&unattended, submit.clone()).await;
    assert!(
        r.is_error && r.content.contains("owner's yes"),
        "{}",
        r.content
    );
    // The owner's own chat with no card to show: still no.
    let no_card = ToolContext {
        origin: crate::origin::Origin::User,
        session_key: "agent:app-1:web".into(),
        ..Default::default()
    };
    assert!(tool.execute_dyn(&no_card, submit.clone()).await.is_error);
    // The owner says not yet.
    let r = tool
        .execute_dyn(&owner_answers("Not yet"), submit.clone())
        .await;
    assert!(r.is_error && r.content.contains("not yet"), "{}", r.content);
    assert!(
        hub.calls.lock().unwrap().is_empty(),
        "nothing reached the hub: {:?}",
        hub.calls.lock().unwrap()
    );
    assert_eq!(
        store.get_app_listing("app-1").unwrap().unwrap().artifact_id,
        ""
    );

    // The owner taps Submit for review.
    let r = tool
        .execute_dyn(&owner_answers(SUBMIT_ANSWER), submit)
        .await;
    assert!(!r.is_error, "{}", r.content);
    assert_eq!(
        *hub.calls.lock().unwrap(),
        vec![
            "agent:create",
            "agent:bundle-token",
            "upload_bundle",
            "agent:update",
            "agent:submit"
        ]
    );
    let row = store.get_app_listing("app-1").unwrap().unwrap();
    assert_eq!(
        (
            row.artifact_id.as_str(),
            row.status.as_str(),
            row.version.as_str()
        ),
        ("art-1", "in_review", "1.0.0")
    );
    assert_eq!(row.chat_session, "agent:app-1:web");
    assert_eq!(r.payload.as_ref().unwrap()["kind"], "app_listing");
}

/// With the mode off an app works only on itself: its own listing, never
/// another app's, and an app installed from the marketplace not at all.
#[tokio::test]
async fn with_the_mode_off_an_app_publishes_only_itself() {
    let (dir, store) = temp_store();
    seed_app(&store, dir.path(), "app-1", "Kart Racer");
    seed_app(&store, dir.path(), "app-2", "Note Pad");
    seed_app(&store, dir.path(), "app-9", "Bought Game");
    store.set_agent_napp_path("app-9", "/data/nebo/agents/bought-game.napp").unwrap();
    let ctx = |who: &str| ToolContext {
        origin: crate::origin::Origin::User,
        session_key: format!("agent:{who}:web"),
        ..Default::default()
    };
    let publisher = Arc::new(Publisher::new(store.clone()).with_hub(Arc::new(FakeHub::default())));
    let listing = AppListingTool(publisher.clone());
    let mine = listing.execute_dyn(&ctx("app-1"), json!({})).await;
    assert!(!mine.is_error, "{}", mine.content);
    assert!(mine.content.contains("Kart Racer"), "{}", mine.content);

    let other = listing.execute_dyn(&ctx("app-1"), json!({"app": "Note Pad"})).await;
    assert!(other.is_error && other.content.contains("App Developer mode is off"), "{}", other.content);
    let other = AppSubmitTool(publisher).execute_dyn(&ctx("app-1"), json!({"app": "app-2"})).await;
    assert!(other.is_error && other.content.contains("App Developer mode is off"), "{}", other.content);
    let shot = AppScreenshotTool::new(store.clone(), None).with_hub(Arc::new(FakeHub::default()));
    let other = shot.execute_dyn(&ctx("app-1"), json!({"app": "Note Pad"})).await;
    assert!(other.is_error && other.content.contains("App Developer mode is off"), "{}", other.content);

    let bought = listing.execute_dyn(&ctx("app-9"), json!({})).await;
    assert!(bought.is_error && bought.content.contains("installed from the marketplace"), "{}", bought.content);
}

/// A draft call saves the listing and shows it, with what keeps it from
/// being submitted; the owner's change is applied.
#[tokio::test]
async fn a_draft_is_saved_and_shown_with_whats_missing() {
    let (dir, store) = temp_store();
    set_mode(&store, true);
    seed_app(&store, dir.path(), "app-1", "Kart Racer");
    let tool = AppListingTool(Arc::new(
        Publisher::new(store.clone()).with_hub(Arc::new(FakeHub::default())),
    ));
    let ctx = ToolContext {
        origin: crate::origin::Origin::User,
        session_key: "agent:app-1:web".into(),
        ..Default::default()
    };
    let r = tool
        .execute_dyn(&ctx, json!({"visibility": "unlisted"}))
        .await;
    assert!(!r.is_error, "{}", r.content);
    assert!(
        r.content.contains("**Kart Racer** · v1.0.0 · Unlisted"),
        "{}",
        r.content
    );
    assert!(r.content.contains("no screenshots yet"), "{}", r.content);
    let saved: ListingDraft =
        serde_json::from_str(&store.get_app_listing("app-1").unwrap().unwrap().draft).unwrap();
    assert_eq!(saved.visibility, "unlisted");
    assert_eq!(saved.category.as_deref(), Some("Games"));
}
