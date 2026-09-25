use std::sync::Arc;

use crate::app_tool::AppTool;
use crate::desktop_tool::DesktopTool;
use crate::domain::DomainInput;
use crate::keychain_tool::KeychainTool;
use crate::music_tool::MusicTool;
use crate::organizer;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ResourceKind, ToolResult};
use crate::settings_tool::SettingsTool;
use crate::spotlight_tool::SpotlightTool;

/// The OS tool: desktop automation, apps, settings, media, credentials,
/// search and personal information management under one namespace. Files
/// and commands are their own tools (`file_tools`, `command_tools`).
pub struct OsTool {
    desktop_tool: DesktopTool,
    app_tool: AppTool,
    settings_tool: SettingsTool,
    music_tool: MusicTool,
    keychain_tool: KeychainTool,
    spotlight_tool: SpotlightTool,
    store: Option<Arc<db::Store>>,
    /// To know whether a typed port (`mail.message.send`) has a provider:
    /// when it does, the local mail app steps aside.
    plugin_store: Option<Arc<napp::plugin::PluginStore>>,
}

impl OsTool {
    pub fn new() -> Self {
        Self {
            desktop_tool: DesktopTool::new(),
            app_tool: AppTool::new(),
            settings_tool: SettingsTool::new(),
            music_tool: MusicTool::new(),
            keychain_tool: KeychainTool::new(),
            spotlight_tool: SpotlightTool::new(),
            store: None,
            plugin_store: None,
        }
    }

    pub fn with_plugin_store(mut self, ps: Arc<napp::plugin::PluginStore>) -> Self {
        self.plugin_store = Some(ps);
        self
    }

    pub fn with_store(mut self, store: Arc<db::Store>) -> Self {
        self.store = Some(store);
        self
    }

    /// The current-set tool name for the job a call does (its permission
    /// rule key): the desktop and organizer tools this call stands for.
    pub fn rule_key_for(input: &serde_json::Value) -> String {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let named = |family: &str| {
            if action.is_empty() {
                family.to_string()
            } else {
                format!("{family}_{action}")
            }
        };
        match OsTool::resolved_resource(input) {
            "capture" => match action {
                "see" => "desktop_see",
                _ => "desktop_screenshot",
            }
            .to_string(),
            "input" => match action {
                "click" | "double_click" | "right_click" | "" => "desktop_click".to_string(),
                "move" => "desktop_move_mouse".to_string(),
                "hotkey" | "press" => "desktop_key".to_string(),
                other => format!("desktop_{other}"),
            },
            "tts" => "speak".to_string(),
            "settings" => "system_settings".to_string(),
            "music" => "music_control".to_string(),
            "search" => "search_computer".to_string(),
            "window" | "clipboard" | "ui" | "menu" | "dialog" | "space" | "shortcut" | "dock"
            | "app" | "keychain" => named(OsTool::resolved_resource(input)),
            "mail" => match action {
                "send" => "mail_message_send",
                "search" => "mail_inbox_search",
                "accounts" => "mail_accounts",
                _ => "mail_inbox_read",
            }
            .to_string(),
            "calendar" => match action {
                "create" => "calendar_event_create",
                "update" => "calendar_event_update",
                "delete" => "calendar_event_cancel",
                "get" => "calendar_event_get",
                "availability" => "calendar_availability_get",
                "list" | "today" | "upcoming" => "calendar_event_list",
                _ => return named("calendar"),
            }
            .to_string(),
            "contacts" => match action {
                "search" | "list" => "contacts_search".to_string(),
                _ => named("contacts"),
            },
            "reminders" => match action {
                "create" => "reminders_create".to_string(),
                "complete" => "reminders_complete".to_string(),
                "delete" => "reminders_delete".to_string(),
                "lists" => "reminders_lists".to_string(),
                _ => "reminders_list".to_string(),
            },
            "notification" => "push_notification".to_string(),
            _ => "os".to_string(),
        }
    }

    /// An os call's owner-facing lines say WHAT was done. "Checked the
    /// workspace ×17" hid a model tapping the same point seven times and
    /// never looking (2026-09-19); the command, the app and the point are what
    /// the owner needs to see.
    pub(crate) fn labels(input: &serde_json::Value) -> (String, String) {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let app = input.get("app").and_then(|v| v.as_str()).unwrap_or("");
        let point = input
            .get("coordinate")
            .and_then(|v| v.as_array())
            .filter(|a| a.len() == 2)
            .map(|a| format!("({},{})", a[0], a[1]))
            .or_else(|| Some(format!("({},{})", input.get("x")?.as_i64()?, input.get("y")?.as_i64()?)));
        let labelled = match action {
            "click" | "double_click" | "right_click" => {
                let what = input
                    .get("ref")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .or(point)
                    .unwrap_or_default();
                let where_ = if app.is_empty() { what } else { format!("{what} in {app}") };
                Some((format!("clicking {where_}"), format!("Clicked {where_}")))
            }
            "screenshot" | "see" | "capture" => {
                let what = if !app.is_empty() {
                    app.to_string()
                } else if let Some(r) = input.get("region").and_then(|v| v.as_str()) {
                    format!("region {r}")
                } else {
                    "the screen".to_string()
                };
                Some((format!("capturing {what}"), format!("Captured {what}")))
            }
            "activate" | "launch" if !app.is_empty() => Some((format!("opening {app}"), format!("Opened {app}"))),
            _ => None,
        };
        labelled.unwrap_or_else(|| crate::humanize::call_labels("os", input))
    }


    /// Resolve the effective resource of an os call — THE canonical chain:
    /// explicit non-empty `resource` field → [`Self::infer_resource`] from the
    /// action name → [`Self::infer_resource_from_context`] from the parameters.
    /// Every consumer (approval gate, resource permits, concurrency, capability
    /// gating, safeguards, path scoping) must use this so they all agree on
    /// which resource a call targets.
    /// The resource a call operates on, inferring it when the model omitted the
    /// field. PUBLIC because it is the ONE definition of that inference — the
    /// history trim (`agent::harness::compact::trim`) must classify a call exactly as the
    /// executor did, or it mislabels the call and can destroy its result
    /// (2026-08-28: a bare `os` call was summarized as `[os] 0 lines` and the
    /// model believed its result was empty).
    pub fn resolved_resource(input: &serde_json::Value) -> &str {
        let resource = input.get("resource").and_then(|v| v.as_str()).unwrap_or("");
        if !resource.is_empty() {
            return resource;
        }
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        // Parameters settle an action that several resources share before its
        // bare name does.
        let shared = Self::infer_resource_from_shared_action(action, input);
        if !shared.is_empty() {
            return shared;
        }
        let inferred = Self::infer_resource(action);
        if inferred.is_empty() {
            Self::infer_resource_from_context(input)
        } else {
            inferred
        }
    }

    /// Actions one resource owns by name that another resource also uses,
    /// settled by the parameters the call carries. Each arm is a misroute the
    /// 2026-09-05 audit found live: a window `move` with `app` went to the
    /// mouse and a notification `send` went to Mail (and its approval gate).
    pub(crate) fn infer_resource_from_shared_action(
        action: &str,
        input: &serde_json::Value,
    ) -> &'static str {
        let has = |k: &str| {
            input
                .get(k)
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
        };
        let has_input_target = has("ref")
            || has("element_id")
            || has("element")
            || input.get("coordinate").is_some()
            || input.get("x").is_some();
        match action {
            // `account` alone is safe here: keychain uses get/find, never "read".
            "read" => {
                let ctx = Self::infer_resource_from_context(input);
                if !ctx.is_empty() {
                    ctx
                } else if has("account") {
                    "mail"
                } else {
                    ""
                }
            }
            "move" if has("app") => "window",
            // `action: "menu", name: "Edit > Find"` means choose that item.
            "menu" if has("name") => "menu",
            // "Edit > Select All" is a menu path, whatever resource was left out.
            "click" if input.get("name").and_then(|v| v.as_str()).is_some_and(|n| n.contains('>')) => "menu",
            "list" if has("app") && has("name") => "menu",
            "click" if has("name") => "dialog",
            // A click by label resolves against the last capture (see input
            // `target`), not the AppleScript UI path.
            "click" if has("label") => "input",
            "click" if has("role") => "ui",
            "click" if has("app") && !has_input_target => "ui",
            // A `find` inside a named app looks for an element, not a secret.
            "find" if has("app") => "ui",
            "send" if input.get("to").is_none() && (has("title") || has("message")) => {
                "notification"
            }
            _ => "",
        }
    }

    /// Infer resource from action name when resource field is omitted.
    pub(crate) fn infer_resource(action: &str) -> &str {
        match action {
            // Input
            "click" | "type" | "press" | "move" | "double_click" | "right_click" | "hotkey"
            | "scroll" | "drag" | "paste" => "input",
            // Capture ("capture" is what the desktop straps call a screenshot)
            "screenshot" | "see" | "capture" | "wait" => "capture",
            // Settings: every setting is its own action name
            "volume" | "brightness" | "mute" | "unmute" | "wifi" | "bluetooth" | "darkmode"
            | "battery" => "settings",
            // Music
            "play" | "pause" | "next" | "previous" | "shuffle" | "playlists" => "music",
            // App
            "launch" | "quit" | "quit_all" | "activate" | "hide" | "frontmost" => "app",
            // TTS
            "speak" => "tts",
            // Organizer inferences
            "accounts" | "unread" | "send" => "mail",
            "today" | "upcoming" | "calendars" | "configure" | "pending" | "accept" | "decline"
            | "auto_accept" => "calendar",
            "groups" => "contacts",
            "lists" | "complete" => "reminders",
            _ => "",
        }
    }

    /// Infer resource from parameter context when action-based inference fails
    /// (e.g. "create" is shared across calendar, contacts, reminders).
    pub(crate) fn infer_resource_from_context(input: &serde_json::Value) -> &'static str {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        // Keychain: a `password` param is uniquely keychain-shaped, and
        // `service` with a keychain verb is too — models often write the full
        // arg set (service/account/password) and drop `resource`, which used
        // to cost three "Resource is required" errors before the first store.
        let has_kc_field = |key: &str| {
            input
                .get(key)
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
        };
        if has_kc_field("password")
            || ((has_kc_field("service") || has_kc_field("label"))
                && matches!(action, "get" | "find" | "add" | "store" | "delete"))
        {
            return "keychain";
        }
        // Calendar: date, calendar, end_date, location, or days present
        if input
            .get("date")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty())
            || input
                .get("calendar")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
            || input
                .get("end_date")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
            || input.get("days").is_some()
        {
            return "calendar";
        }
        // Reminders: list, due_date, or priority present
        if input
            .get("list")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty())
            || input
                .get("due_date")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
            || input.get("priority").is_some()
        {
            return "reminders";
        }
        // Contacts: email, phone, or company present
        if input
            .get("phone")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty())
            || input
                .get("company")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
            || input
                .get("email")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
        {
            return "contacts";
        }
        // Mail: to, cc, subject, or mailbox present
        if input.get("to").is_some()
            || input
                .get("subject")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
            || input
                .get("mailbox")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
        {
            return "mail";
        }
        ""
    }

    /// Every resource the os tool dispatches to.
    const RESOURCE_NAMES: &'static [&'static str] = &[
        "window", "input", "clipboard", "capture", "notification",
        "ui", "menu", "dialog", "space", "shortcut", "tts", "dock",
        "app", "settings", "music", "keychain", "search",
        "mail", "contacts", "calendar", "reminders",
    ];

    /// The call as it will run: shorthand accepted (first-call doctrine: fix
    /// the API, not the client) and the resource it resolves to written into
    /// `resource`.
    ///
    /// This is the ONE place a call's shape is settled, and the registry
    /// applies it (`DynTool::normalize_input`) BEFORE any gate reads the
    /// call: origin deny list, capability and approval all see the action and resource that execute, whichever shape the
    /// model wrote. Idempotent, so running it again changes nothing.
    pub(crate) fn normalized(input: serde_json::Value) -> serde_json::Value {
        let mut v = input;
        // A call with no action is refused by the tool.
        let Ok(domain_input) = serde_json::from_value::<DomainInput>(v.clone()) else {
            return v;
        };
        let corrected =
            crate::domain::auto_correct_resource(&domain_input, &mut v, Self::RESOURCE_NAMES);
        let resource = if corrected.is_empty() {
            Self::resolved_resource(&v).to_string()
        } else {
            corrected
        };
        if resource.is_empty() {
            return v;
        }
        // Settings VALUES models guess as resources: `os(resource:
        // "battery", action: "info")` is the natural first shape, but
        // battery/volume/brightness are ACTIONS on the settings
        // resource. Honor the guess instead of erroring.
        let resource = if matches!(resource.as_str(), "battery" | "volume" | "brightness") {
            v["action"] = serde_json::Value::String(resource);
            "settings".to_string()
        } else {
            resource
        };
        v["resource"] = serde_json::Value::String(resource);
        v
    }

    /// The actions the parse error lists when a call names none and the
    /// fields do not settle it.
    const ACTION_INDEX: &'static str = "app launch/quit/activate/list; \
         capture screenshot/see; input click/type/press/scroll; \
         settings volume/brightness/mute/wifi/bluetooth/darkmode/battery; \
         mail unread/read/send/search; calendar today/upcoming/create; \
         reminders lists/list/create/complete; contacts search/get/create";

    /// The settings tool's call for an os settings action: the os action IS
    /// the setting name (volume, wifi, ...) and the presence of `value`
    /// decides get/set or status/toggle. `unmute` is `mute` with value false:
    /// one setting, one handler.
    pub(crate) fn settings_call(input: &serde_json::Value) -> Result<serde_json::Value, String> {
        let action = input["action"].as_str().unwrap_or("");
        let has_value = input.get("value").is_some_and(|v| !v.is_null());
        let mut call = input.clone();
        let (setting, settings_action) = match action {
            "sleep" | "lock" | "mute" => (action, "trigger"),
            "unmute" => {
                call["value"] = serde_json::json!(false);
                ("mute", "trigger")
            }
            "volume" | "brightness" => (action, if has_value { "set" } else { "get" }),
            "wifi" | "bluetooth" | "darkmode" => (action, if has_value { "toggle" } else { "status" }),
            "battery" | "info" => (action, "get"),
            other => {
                return Err(format!(
                    "Unknown setting '{other}'. Use: volume, brightness, wifi, bluetooth, battery, \
                     darkmode, sleep, lock, info, mute (value: true|false), unmute"
                ));
            }
        };
        call["resource"] = serde_json::json!(setting);
        call["action"] = serde_json::json!(settings_action);
        Ok(call)
    }

}

impl Default for OsTool {
    fn default() -> Self {
        Self::new()
    }
}

impl DynTool for OsTool {
    fn name(&self) -> &str {
        "os"
    }

    fn description(&self) -> String {
        // Said up front in server mode: a gate run (2026-09-23) watched the
        // model call os(action: "today", calendar: …) on a headless server
        // and get the refusal below after the fact.
        let server_note = if crate::server_mode() {
            "SERVER MODE — this Nebo runs in the cloud: no mail, contacts, calendar, reminders, notification, shortcut, tts or dock (never call them here); window, input, clipboard, capture, ui, menu, dialog and space only while a desktop session is up. Keychain, settings and search work normally.\n\n"
        } else {
            ""
        };
        format!("{server_note}{}", "Local machine operations — apps, desktop automation, settings, media, credentials, search, PIM. Files and commands have their own tools (read_file, edit_file, write_file, run_command).\n\n\
         Rules:\n\
         - Always pass `action`. `resource` is inferred when the action belongs to one resource (play→music, volume→settings) or its parameters settle it (move+app→window, click+label→input (resolved against the last capture), send+title→notification); pass it for actions several resources share (create, list, search, get, delete).\n\n\
         Resources:\n\
         - window: list, focus, minimize, maximize, resize, close, move\n\
         - input: click, double_click, right_click, type, press, hotkey, move, scroll, drag, paste — by ref through accessibility; right_click on a [menu] element opens its context menu and lists the items as refs; every act returns the window after it and says whether it was delivered and what changed; wait_for waits for text/an element/a menu instead of guessing a pause\n\
         - clipboard: read, write, clear\n\
         - capture: screenshot, see (ref: drills into a +N container), wait (app + text | label | gone | menu | window)\n\
         - notification: send, alert\n\
         - ui: tree, find, click, get_value, set_value, list_apps\n\
         - menu: list (name: \"File\" lists that menu), menus, click (name: \"File > Export…\"), status, click_status — the app's menu bar, read and pressed through accessibility\n\
         - dialog: detect, list, click, fill, dismiss\n\
         - space: list, switch, move_window\n\
         - shortcut: list, run\n\
         - tts: speak\n\
         - dock: badges, recent, is_running (macOS)\n\
         - app: list, launch, quit, quit_all, activate, hide, info, frontmost\n\
         - settings: volume, brightness, wifi, bluetooth, battery, darkmode, sleep, lock, info, mute (value: true|false), unmute\n\
         - music: play, pause, next, previous, status, search, volume, playlists, shuffle\n\
         - keychain: get, find, add (alias: store), delete (account optional — narrows the match)\n\
         - search: search (file search via OS index)\n\
         - mail: accounts, unread, read, send, search — LOCAL Apple Mail. send takes to, subject, text (the message, plain) and optional html; it is the way to send only when no mail plugin is connected — a connected one is the business's mail and this send refuses and points to mail_message_send. read/search take optional account (name or address, e.g. \"you@example.com\") + mailbox; search is a SUBSTRING match on subject/sender (no Gmail operators like from:)\n\
         - contacts: search, get, create, groups\n\
         - calendar: calendars, today, upcoming, create, delete, pending, accept, decline, auto_accept, list, configure — the LOCAL Apple/Mac calendar (a connected calendar service is its own plugin__<name> tool)\n\
         - reminders: lists, list, create, complete, delete\n\n\
         Examples:\n  \
         os(resource: \"app\", action: \"launch\", app: \"Safari\")\n  \
         os(resource: \"capture\", action: \"screenshot\")\n  \
         os(resource: \"capture\", action: \"see\", app: \"Safari\") — returns snapshot_id + element IDs\n  \
         os(resource: \"input\", action: \"click\", ref: \"B3\") — click element from snapshot (or coordinate: [x, y])\n  \
         os(resource: \"input\", action: \"type\", ref: \"T1\", text: \"hello\") — focus + type\n  \
         os(resource: \"music\", action: \"play\")\n  \
         os(resource: \"keychain\", action: \"get\", service: \"myapp\", account: \"user@example.com\")\n  \
         os(resource: \"mail\", action: \"unread\")"
            .to_string())
    }

    fn schema(&self) -> serde_json::Value {
        // Built programmatically to avoid serde_json::json! recursion limit
        let mut props = serde_json::Map::new();

        let prop = |t: &str, d: &str| -> serde_json::Value {
            serde_json::json!({"type": t, "description": d})
        };

        props.insert(
            "resource".into(),
            serde_json::json!({
                "type": "string",
                "description": "Optional. The resource category — usually inferred from the action (play→music, volume→settings). Specify it only to disambiguate actions shared across resources (e.g. create, list).",
                "enum": [
                    "window", "input", "clipboard", "capture", "notification",
                    "ui", "menu", "dialog", "space", "shortcut", "tts", "dock",
                    "app", "settings", "music", "keychain", "search",
                    "mail", "contacts", "calendar", "reminders"
                ]
            }),
        );
        props.insert(
            "action".into(),
            prop("string", "The operation to perform on the selected resource (e.g. resource: \"calendar\" → action: \"today\"). Never put a resource name here."),
        );
        props.insert(
            "limit".into(),
            prop("integer", "Max results to return"),
        );
        props.insert(
            "filter".into(),
            prop("string", "Substring filter for a ui or app list"),
        );
        // Desktop
        props.insert("app".into(), prop("string", "Application name"));
        props.insert(
            "title".into(),
            prop("string", "Window or notification title"),
        );
        props.insert("message".into(), prop("string", "Notification message"));
        props.insert("text".into(), prop("string", "The text: a mail send's message (plain text), or text to type, write, or speak for desktop input/tts."));
        props.insert("html".into(), prop("string", "Optional HTML version of a mail send's message, where the provider can send one (Outlook). Mail.app and the Linux clients send plain text and refuse it."));
        props.insert("key".into(), prop("string", "Key to press"));
        props.insert("keys".into(), prop("string", "Key combination for hotkey"));
        props.insert("x".into(), prop("integer", "X coordinate for window move. Input actions take coordinate: [x, y] (x and y are read there too)"));
        props.insert("y".into(), prop("integer", "Y coordinate for window move"));
        props.insert(
            "coordinate".into(),
            serde_json::json!({
                "type": "array",
                "items": { "type": "integer" },
                "description": "Input click/move/type target as [x, y] on screen, when there is no element ref; drag end point"
            }),
        );
        props.insert(
            "start_coordinate".into(),
            serde_json::json!({
                "type": "array",
                "items": { "type": "integer" },
                "description": "Input drag start point as [x, y]"
            }),
        );
        props.insert("x2".into(), prop("integer", "End X coordinate (drag)"));
        props.insert("y2".into(), prop("integer", "End Y coordinate (drag)"));
        props.insert("dx".into(), prop("integer", "Scroll delta X"));
        props.insert("dy".into(), prop("integer", "Scroll delta Y"));
        props.insert("width".into(), prop("integer", "Width for resize/move"));
        props.insert("height".into(), prop("integer", "Height for resize/move"));
        props.insert(
            "region".into(),
            prop("string", "Screenshot region: 'x,y,w,h'"),
        );
        props.insert(
            "quality".into(),
            prop("string", "Screenshot quality: 'low' (800px JPEG), 'medium' (1280px JPEG, default), 'high' (full-res PNG)"),
        );
        props.insert(
            "name".into(),
            prop("string", "Name for shortcut/menu/contact/reminder"),
        );
        props.insert("value".into(), prop("string", "Value to set"));
        props.insert("role".into(), prop("string", "UI element role filter"));
        props.insert("label".into(), prop("string", "UI element label"));
        props.insert("index".into(), prop("integer", "Index for space/menu"));
        props.insert("voice".into(), prop("string", "TTS voice name"));
        props.insert("rate".into(), prop("integer", "TTS speaking rate"));
        // Snapshot (see → click flow)
        props.insert(
            "ref".into(),
            prop(
                "string",
                "Input click/type/move target: the element ref from capture(action: see) (e.g. B1, T2). On capture see: drill into that element (a container marked +N inside) and list only its contents",
            ),
        );
        props.insert(
            "wait_for".into(),
            serde_json::json!({
                "type": "object",
                "description": "Input actions and capture wait: wait for something instead of a fixed pause — {text: \"Saved\"} | {appears: \"Export\"} | {gone: \"Loading\"} | {menu: true|false} | {window: true | \"title\"}, optional timeout_ms (default 5000, max 30000)"
            }),
        );
        props.insert(
            "repeat".into(),
            prop("integer", "Input press: press the key this many times (max 30)"),
        );
        props.insert(
            "target".into(),
            prop("string", "Input click/type/right_click: the element in words (\"the Save button\") instead of a ref — resolved against the last capture: one element with exactly that label is used; otherwise Jev picks, and when it is not sure the call comes back with the reason and you choose the ref"),
        );
        props.insert(
            "physical".into(),
            prop("boolean", "Input click/type: use the real mouse and keyboard instead of accessibility. Off by default; an accessibility action that fails is reported, never silently replaced"),
        );
        props.insert(
            "force".into(),
            prop("boolean", "Input press: send a combo that logs out, locks or force-quits (refused without it)"),
        );
        props.insert(
            "element_id".into(),
            prop("string", "Alias of ref"),
        );
        props.insert(
            "snapshot_id".into(),
            prop("string", "Snapshot ID from a previous see action"),
        );
        props.insert(
            "max_elements".into(),
            prop("integer", "Max elements returned by see (default: 100)"),
        );
        // Keychain
        props.insert("service".into(), prop("string", "Keychain service name"));
        props.insert("account".into(), prop("string", "Keychain account, or Mail account filter (name or address) for mail read/search"));
        props.insert("password".into(), prop("string", "Password to store"));
        // Search
        props.insert("query".into(), prop("string", "Search query"));
        props.insert("dir".into(), prop("string", "Directory to search within"));
        // Organizer
        props.insert("email".into(), prop("string", "Email address"));
        props.insert("subject".into(), prop("string", "Email subject"));
        props.insert(
            "to".into(),
            serde_json::json!({
                "oneOf": [
                    { "type": "string" },
                    { "type": "array", "items": { "type": "string" } }
                ],
                "description": "mail send: the recipient address(es)."
            }),
        );
        props.insert(
            "cc".into(),
            serde_json::json!({
                "type": "array",
                "items": { "type": "string" },
                "description": "CC recipient(s)"
            }),
        );
        props.insert(
            "mailbox".into(),
            prop("string", "Mailbox name (e.g. 'INBOX', 'Sent')"),
        );
        props.insert("calendar".into(), prop("string", "Calendar name"));
        props.insert(
            "date".into(),
            prop("string", "Start date (e.g. '2025-03-15 10:00', 'tomorrow')"),
        );
        props.insert(
            "end_date".into(),
            prop("string", "End date (defaults to start + 1 hour)"),
        );
        props.insert("location".into(), prop("string", "Event location"));
        props.insert(
            "days".into(),
            prop("integer", "Number of days to look ahead (default: 7)"),
        );
        props.insert("list".into(), prop("string", "Reminder list name"));
        props.insert(
            "due_date".into(),
            prop(
                "string",
                "Due date (e.g. '2025-03-15', 'tomorrow', 'in 3 days')",
            ),
        );
        props.insert(
            "priority".into(),
            prop("integer", "Priority: 1-3=high, 4-6=medium, 7-9=low"),
        );
        props.insert("phone".into(), prop("string", "Contact phone number"));
        props.insert(
            "company".into(),
            prop("string", "Contact company/organization"),
        );
        props.insert("notes".into(), prop("string", "Notes or description"));

        serde_json::json!({
            "type": "object",
            "properties": serde_json::Value::Object(props),
            "required": ["action"]
        })
    }


    fn normalize_input(&self, input: serde_json::Value) -> serde_json::Value {
        Self::normalized(input)
    }


    fn resource_permit(&self, input: &serde_json::Value) -> Option<ResourceKind> {
        match OsTool::resolved_resource(input) {
            // Physical screen resources — one mouse, one keyboard, one display
            "window" | "input" | "ui" | "menu" | "dialog" | "space" | "shortcut" => {
                Some(ResourceKind::Screen)
            }
            // Parallelizable: capture, app, clipboard, notification, tts, dock, file,
            // shell, settings, music, keychain, search, mail, contacts, calendar, reminders
            _ => None,
        }
    }

    fn execution_timeout(&self, input: &serde_json::Value) -> Option<std::time::Duration> {
        // A file search stops itself well inside its own deadline. Hand the
        // engine that same budget so the search's plain sentence about
        // narrowing the query is what reaches the model — the runner's
        // generic 300 s timeout text never can, because the harness ends a
        // silent run at 180 s. Every other resource keeps the default.
        if OsTool::resolved_resource(input) == "search" {
            return self.spotlight_tool.execution_timeout(input);
        }
        None
    }

    fn search_hint(&self) -> &str {
        "desktop apps settings mail calendar"
    }

    fn should_defer(&self) -> bool {
        false
    }

    fn read_only(&self, input: &serde_json::Value) -> bool {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        match OsTool::resolved_resource(input) {
            "search" => true,
            "capture" => matches!(action, "screenshot" | "see" | "wait"),
            _ => false,
        }
    }

    fn rule_key(&self, input: &serde_json::Value) -> String {
        Self::rule_key_for(&Self::normalized(input.clone()))
    }

    fn rule_field(&self, input: &serde_json::Value) -> Option<types::permissions::RuleField> {
        let str_of = |k: &str| input.get(k).and_then(|v| v.as_str()).filter(|s| !s.is_empty());
        match OsTool::resolved_resource(input) {
            "mail" => str_of("to").map(|t| types::permissions::RuleField::Recipient(t.to_string())),
            _ => None,
        }
    }

    fn capability(&self, input: &serde_json::Value) -> Option<&'static str> {
        match OsTool::resolved_resource(input) {
            "settings" | "keychain" | "platform" | "system" => Some("system"),
            "capture" | "screenshot" | "see" => Some("media"),
            "contacts" => Some("contacts"),
            // Mail, calendar and reminders are not behind a coarse toggle.
            "mail" | "calendar" | "reminders" => None,
            // Everything else is desktop control.
            _ => Some("desktop"),
        }
    }

    fn effects(&self, input: &serde_json::Value) -> types::permissions::CallEffects {
        use types::permissions::{CallEffects, Knowable};
        if self.read_only(input) {
            return CallEffects::none();
        }
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        match (OsTool::resolved_resource(input), action) {
            ("mail", "send") => CallEffects {
                recipients: input
                    .get("to")
                    .and_then(|v| v.as_str())
                    .map(|t| t.split(',').map(|r| r.trim().to_string()).filter(|r| !r.is_empty()).collect())
                    .unwrap_or_default(),
                publishes: Knowable::No,
                ..CallEffects::default()
            },
            _ => CallEffects::unknown(),
        }
    }

    fn taint(&self, input: &serde_json::Value) -> Option<types::provenance::ProvenanceClass> {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        match OsTool::resolved_resource(input) {
            // Mailbox reads bring in other people's words.
            "mail" if !matches!(action, "send" | "accounts") => {
                Some(types::provenance::ProvenanceClass::ExternalEmail)
            }
            _ => None,
        }
    }

    fn emits_image(&self, input: &serde_json::Value) -> bool {
        // Reading an existing image returns it for the model, and every
        // observe and click returns the window it acted on: the tool's eyes,
        // not media the owner asked for. An explicit screenshot is.
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        !matches!(
            action,
            "read" | "see" | "find" | "click" | "double_click" | "right_click" | "type" | "press"
                | "hotkey" | "move" | "scroll" | "drag" | "paste"
        )
    }

    fn activity(&self, input: &serde_json::Value) -> String {
        Self::labels(input).0
    }

    fn outcome(&self, input: &serde_json::Value) -> String {
        Self::labels(input).1
    }

    /// Pre-interface: it settles its own call shapes (see
    /// `DynTool::validates_input`).
    fn validates_input(&self) -> bool {
        false
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let input = Self::normalized(input);
            let domain_input: DomainInput = match serde_json::from_value(input.clone()) {
                Ok(v) => v,
                Err(e) => {
                    let keys = input
                        .as_object()
                        .map(|o| o.keys().cloned().collect::<Vec<_>>().join(", "))
                        .unwrap_or_default();
                    return ToolResult::error(format!(
                        "Failed to parse input: {e}. Received fields: [{keys}]. Every `os` \
                         call needs an `action` (resource is inferred when omitted). Actions: \
                         {}. E.g. os(resource: \"app\", action: \"launch\", app: \"Safari\").",
                        Self::ACTION_INDEX
                    ));
                }
            };

            // `normalized` wrote the resource when the call settles one.
            let resource = input
                .get("resource")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if resource.is_empty() {
                return ToolResult::error(format!(
                    "Could not infer a resource from action '{}'. Pass resource explicitly (window, input, clipboard, capture, notification, ui, menu, dialog, space, shortcut, \
                     tts, dock, app, settings, music, keychain, search, mail, contacts, calendar, \
                     reminders) or use one of the documented actions.",
                    domain_input.action
                ));
            }

            // Desktop-bound resources have no counterpart in a cloud deploy —
            // no screen, input devices, or Mail/Calendar apps. Refuse with a
            // reason the model can act on, instead of letting the platform
            // layer fail deep inside with a cryptic xdotool/Evolution error.
            // file/shell/web/search all work normally here, so only these are
            // gated.
            //
            // Exception: while the bot's on-demand desktop session is live
            // (the "computer" — Xvfb + xfce in this pod), the X11-backed
            // resources work against that display and are un-gated. Apps that
            // simply aren't in the image (Mail/Calendar/tts…) stay gated even
            // with a session up.
            if crate::server_mode() {
                let x11_backed = matches!(
                    resource.as_str(),
                    "window"
                        | "input"
                        | "clipboard"
                        | "capture"
                        | "ui"
                        | "menu"
                        | "dialog"
                        | "space"
                );
                let never_in_cloud = matches!(
                    resource.as_str(),
                    "notification"
                        | "shortcut"
                        | "tts"
                        | "dock"
                        | "mail"
                        | "contacts"
                        | "calendar"
                        | "reminders"
                );
                if never_in_cloud || (x11_backed && !crate::desktop_session::active()) {
                    // A mail send with the message in the wrong field is
                    // refused for that reason on every platform: the call's
                    // shape is the model's mistake to fix before anything
                    // else, here or on a desktop.
                    if resource == "mail" && input["action"].as_str() == Some("send") {
                        if let Some(why) = mail_send_refusal(&input) {
                            return ToolResult::error(why);
                        }
                    }
                    return ToolResult::error(format!(
                        "os(resource: \"{resource}\") is not available in server mode — this Nebo runs in the cloud and has no screen, input devices, or desktop apps. The file, command and web tools work normally."
                    ));
                }
            }

            match resource.as_str() {
                // Desktop resources — delegate to DesktopTool
                "window" | "input" | "clipboard" | "capture" | "notification" | "ui" | "menu"
                | "dialog" | "space" | "shortcut" | "tts" | "dock" => {
                    self.desktop_tool.execute_dyn(ctx, input).await
                }

                // App lifecycle
                "app" => self.app_tool.execute_dyn(ctx, input).await,

                // Settings: the os action is the setting name; see settings_call.
                "settings" => match Self::settings_call(&input) {
                    Ok(settings_input) => self.settings_tool.execute_dyn(ctx, settings_input).await,
                    Err(msg) => ToolResult::error(msg),
                },

                // Music
                "music" => self.music_tool.execute_dyn(ctx, input).await,

                // Keychain
                "keychain" => self.keychain_tool.execute_dyn(ctx, input).await,

                // File search
                "search" => self.spotlight_tool.execute_dyn(ctx, input).await,

                // PIM — parse OrganizerInput and dispatch to handler functions directly
                "mail" | "contacts" | "calendar" | "reminders" => {
                    let keys = input
                        .as_object()
                        .map(|o| o.keys().cloned().collect::<Vec<_>>().join(", "))
                        .unwrap_or_default();
                    let parsed: organizer::OrganizerInput = match serde_json::from_value(input.clone()) {
                        Ok(v) => v,
                        Err(e) => {
                            return ToolResult::error(format!(
                                "Failed to parse input: {e}. Received fields: [{keys}]. Every `os` \
                                 {resource} call needs an `action`, e.g. \
                                 os(resource: \"mail\", action: \"unread\") or \
                                 os(resource: \"calendar\", action: \"today\")."
                            ));
                        }
                    };
                    match resource.as_str() {
                        // A mail send is a customer-facing effect: it goes through
                        // the ledger, which records it before it runs and never runs
                        // the same one twice. No ledger, no send.
                        "mail" if parsed.action == "send" => {
                            // One field for the message, and an error that names the
                            // mistake: a model that wrote `text` once sent a customer an
                            // empty email.
                            if let Some(why) = mail_send_refusal(&input) {
                                return ToolResult::error(why);
                            }
                            let Some(store) = self.store.as_deref() else {
                                return ToolResult::error("This install has no send ledger; not sent.");
                            };
                            // A connected mail plugin is the business's mail; the
                            // desktop app is the way only when there is none. Which
                            // one is decided here, by what is connected — never by
                            // the model.
                            if let Some(ps) = self.plugin_store.as_deref() {
                                let bound = crate::plugin_tool::bound_providers(ps, store, "mail.message.send");
                                if !bound.is_empty() {
                                    return ToolResult::error(format!(
                                        "Not sent through Apple Mail: this business sends mail through {}. Call mail_message_send with to, subject, text and html — same message, the connected account.",
                                        bound.join(", ")
                                    ));
                                }
                            }
                            let exact = serde_json::json!({
                                "to": &parsed.to, "cc": &parsed.cc, "subject": &parsed.subject,
                                "text": &parsed.text, "html": &parsed.html, "account": &parsed.account,
                            });
                            crate::effects::guarded_send(
                                store,
                                ctx,
                                "messaging",
                                "mail-app",
                                "mail.message.send",
                                &exact,
                                || organizer::mail_send(&parsed),
                            )
                            .await
                        }
                        "mail" => organizer::handle_mail(&parsed.action, &parsed).await,
                        "contacts" => organizer::handle_contacts(&parsed.action, &parsed).await,
                        "calendar" => {
                            organizer::handle_calendar(
                                &parsed.action,
                                &parsed,
                                ctx,
                                self.store.as_ref(),
                            )
                            .await
                        }
                        "reminders" => organizer::handle_reminders(&parsed.action, &parsed).await,
                        _ => unreachable!(),
                    }
                }

                // Resources that live on OTHER tools: redirect with the exact
                // call, so a wrong-tool guess costs one corrected call, not a
                // hunt. (These are the names models actually reach for here.)
                res @ ("context" | "memory" | "session" | "task" | "profile" | "advisors") => {
                    ToolResult::error(format!(
                        "'{res}' is not an os resource — it lives on the `agent` tool. \
                         Call agent(resource: \"{res}\", action: ...) instead."
                    ))
                }
                other => ToolResult::error(format!(
                    "Unknown resource '{}'. Available: file, shell, window, input, clipboard, capture, \
                     notification, ui, menu, dialog, space, shortcut, tts, dock, app, settings, music, \
                     keychain, search, mail, contacts, calendar, reminders",
                    other
                )),
            }
        })
    }
}

/// Why a mail send cannot go out as written, or `None` when the message is
/// where it belongs. ONE field for the message, and an error that names the
/// mistake: a model that wrote `body` once sent a customer an empty email.
/// Checked before anything else about the send — on a desktop or in the
/// cloud — because the call's shape is the model's to fix first.
fn mail_send_refusal(input: &serde_json::Value) -> Option<String> {
    let text = input["text"].as_str().unwrap_or("").trim();
    if !text.is_empty() {
        return None;
    }
    let misnamed = ["body", "message", "content"].into_iter().find(|k| input.get(k).is_some());
    let html = input["html"].as_str().unwrap_or("").trim();
    Some(match misnamed {
        Some(k) => format!("Not sent: `{k}` is not a field of mail send, so the message would have gone out empty. The message goes in `text` (plain), with `html` alongside it if you have a formatted version). Call again with text."),
        None if !html.is_empty() => "Not sent: `text` is required — the plain message every client can read. `html` rides alongside it, never instead of it.".to_string(),
        None => "Not sent: the message has no text. The message goes in `text`.".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every row of the 2026-09-05 misroute table (audit class C, os) plus
    /// the arms that already existed: the resource a call resolves to when
    /// it names none. One table so a dropped arm shows up as one row.
    #[test]
    fn resource_inference_table() {
        let cases: &[(serde_json::Value, &str)] = &[
            // Parameters settle a shared action name.
            (serde_json::json!({"action": "move", "app": "Safari", "x": 0, "y": 0}), "window"),
            (serde_json::json!({"action": "move", "coordinate": [10, 10]}), "input"),
            (serde_json::json!({"action": "click", "app": "Safari", "label": "OK"}), "input"),
            (serde_json::json!({"action": "click", "app": "Safari", "role": "AXButton"}), "ui"),
            (serde_json::json!({"action": "click", "role": "AXButton"}), "ui"),
            (serde_json::json!({"action": "click", "app": "Safari"}), "ui"),
            (serde_json::json!({"action": "click", "app": "Safari", "ref": "B3"}), "input"),
            (serde_json::json!({"action": "click", "name": "OK"}), "dialog"),
            (serde_json::json!({"action": "click", "x": 100, "y": 200}), "input"),
            (serde_json::json!({"action": "send", "title": "Done", "message": "Task complete"}), "notification"),
            (serde_json::json!({"action": "send", "message": "hi"}), "notification"),
            (serde_json::json!({"action": "send", "to": "a@b.c", "subject": "x"}), "mail"),
            (serde_json::json!({"action": "read", "mailbox": "INBOX"}), "mail"),
            // Action names that belong to one resource.
            (serde_json::json!({"action": "volume", "value": 50}), "settings"),
            (serde_json::json!({"action": "brightness"}), "settings"),
            (serde_json::json!({"action": "mute", "value": true}), "settings"),
            (serde_json::json!({"action": "unmute"}), "settings"),
            (serde_json::json!({"action": "battery"}), "settings"),
            (serde_json::json!({"action": "capture"}), "capture"),
            (serde_json::json!({"action": "screenshot"}), "capture"),
            (serde_json::json!({"action": "create", "name": "Ann", "email": "ann@x.com"}), "contacts"),
            // Shared names with nothing to settle them stay unrouted.
            (serde_json::json!({"action": "kill"}), ""),
            (serde_json::json!({"action": "list"}), ""),
            (serde_json::json!({"action": "search", "query": "x"}), ""),
            (serde_json::json!({"action": "get"}), ""),
        ];
        for (input, want) in cases {
            assert_eq!(OsTool::resolved_resource(input), *want, "{input}");
        }
    }

    /// `unmute` is `mute` with value false; the setting name becomes the
    /// settings resource and `value` picks the operation.
    #[test]
    fn settings_call_maps_the_setting_name_and_unmute() {
        let call = OsTool::settings_call(&serde_json::json!({"resource": "settings", "action": "unmute"})).unwrap();
        assert_eq!(call["resource"], "mute");
        assert_eq!(call["action"], "trigger");
        assert_eq!(call["value"], false);
        let call = OsTool::settings_call(&serde_json::json!({"action": "mute"})).unwrap();
        assert_eq!(call["resource"], "mute");
        assert!(call.get("value").is_none(), "mute alone keeps the handler's default (true)");
        let call = OsTool::settings_call(&serde_json::json!({"action": "volume", "value": 50})).unwrap();
        assert_eq!((call["resource"].as_str(), call["action"].as_str()), (Some("volume"), Some("set")));
        let call = OsTool::settings_call(&serde_json::json!({"action": "volume"})).unwrap();
        assert_eq!(call["action"], "get");
        let call = OsTool::settings_call(&serde_json::json!({"action": "wifi"})).unwrap();
        assert_eq!(call["action"], "status");
        let err = OsTool::settings_call(&serde_json::json!({"action": "loudness"})).unwrap_err();
        assert!(err.contains("Unknown setting 'loudness'"), "{err}");
        assert!(err.contains("unmute"), "{err}");
    }

    /// The os schema names the input target the handler reads (ref and
    /// coordinate), keeps element_id as its alias, declares quality, and
    /// describes `to` as the mail recipient.
    #[test]
    fn os_schema_declares_input_targets_quality_and_the_recipient() {
        let schema = os().schema();
        let props = schema["properties"].as_object().expect("object schema");
        for p in ["ref", "coordinate", "start_coordinate", "quality", "element_id"] {
            assert!(props.contains_key(p), "schema is missing `{p}`");
        }
        assert_eq!(props["element_id"]["description"], "Alias of ref");
        let to = props["to"]["description"].as_str().unwrap();
        assert!(to.contains("recipient"), "{to}");
        for gone in ["path", "command", "session_id", "content", "old_string", "pattern", "steps"] {
            assert!(!props.contains_key(gone), "files and commands have their own tools: `{gone}`");
        }
    }

    #[test]
    fn test_infer_resource() {
        assert_eq!(OsTool::infer_resource("read"), "");
        assert_eq!(OsTool::infer_resource("exec"), "");
        assert_eq!(OsTool::infer_resource("click"), "input");
        assert_eq!(OsTool::infer_resource("screenshot"), "capture");
        assert_eq!(OsTool::infer_resource("play"), "music");
        assert_eq!(OsTool::infer_resource("launch"), "app");
        assert_eq!(OsTool::infer_resource("speak"), "tts");
        assert_eq!(OsTool::infer_resource("unread"), "mail");
        assert_eq!(OsTool::infer_resource("today"), "calendar");
        assert_eq!(OsTool::infer_resource("unknown_action"), "");
    }

    /// Stadium 2026-09-23: `find` with `app` and `label` went to the keychain
    /// ("no password stored under Calculator") instead of the UI.
    /// Stadium, 2026-09-24: `click name: "Edit > Select All"` and `list app
    /// name: "Format"` sent without a resource are menu calls.
    #[test]
    fn a_menu_path_or_a_named_menu_routes_to_the_menu() {
        let r = |v: serde_json::Value| OsTool::resolved_resource(&v).to_string();
        assert_eq!(r(serde_json::json!({"action": "click", "name": "Edit > Select All"})), "menu");
        assert_eq!(r(serde_json::json!({"action": "click", "app": "TextEdit", "name": "Edit > Select All"})), "menu");
        assert_eq!(r(serde_json::json!({"action": "list", "app": "TextEdit", "name": "Format"})), "menu");
        assert_eq!(r(serde_json::json!({"action": "click", "name": "OK"})), "dialog");
    }

    #[test]
    fn a_find_inside_an_app_is_a_ui_search_not_a_keychain_lookup() {
        let input = serde_json::json!({"action": "find", "app": "Calculator", "label": "7"});
        assert_eq!(OsTool::resolved_resource(&input), "ui");
        let input = serde_json::json!({"action": "find", "service": "myapp"});
        assert_eq!(OsTool::resolved_resource(&input), "keychain");
    }

    /// AT-09's three "Resource is required" errors: a full keychain arg set
    /// (service/account/password) with `resource` dropped must route itself.
    #[test]
    fn keychain_shaped_args_infer_the_resource() {
        let input = serde_json::json!({
            "action": "add", "service": "myapp", "account": "me", "password": "s3cret"
        });
        assert_eq!(OsTool::resolved_resource(&input), "keychain");
        // A password alone is uniquely keychain-shaped, any verb.
        let input = serde_json::json!({"action": "store", "service": "x", "password": "p"});
        assert_eq!(OsTool::resolved_resource(&input), "keychain");
        // service + keychain verb, no password (get/find/delete legs).
        let input = serde_json::json!({"action": "delete", "service": "myapp"});
        assert_eq!(OsTool::resolved_resource(&input), "keychain");
        // A bare "delete" with a file path must NOT become keychain.
        let input = serde_json::json!({"action": "delete", "path": "/tmp/x"});
        assert_ne!(OsTool::resolved_resource(&input), "keychain");
        // An explicit resource always wins.
        let input = serde_json::json!({"resource": "file", "action": "delete", "service": "x"});
        assert_eq!(OsTool::resolved_resource(&input), "file");
    }

    #[test]
    fn test_infer_resource_from_context_list() {
        // Bare "list" stays ambiguous (window, app, ...)
        let input = serde_json::json!({"action": "list"});
        assert_eq!(OsTool::infer_resource_from_context(&input), "");
        // "list" with a reminders list name still routes to reminders
        let input = serde_json::json!({"action": "list", "list": "Groceries"});
        assert_eq!(OsTool::infer_resource_from_context(&input), "reminders");
    }

    fn os() -> OsTool {
        OsTool::new()
    }

    /// Seen live: a customer received an empty email because the message
    /// was in a field the send did not read. The message is `text`, with
    /// `html` alongside it; a send that puts it anywhere else, or gives only
    /// html, is refused with the mistake named — nothing sent or recorded.
    #[tokio::test]
    async fn a_mail_send_with_the_message_in_the_wrong_field_is_refused_and_steered() {
        let tool = OsTool::new();
        let ctx = crate::origin::ToolContext::default();
        let r = tool.execute_dyn(&ctx, serde_json::json!({"resource": "mail", "action": "send", "to": "a@example.com", "subject": "Re: quote", "body": "hello"})).await;
        assert!(r.is_error, "{}", r.content);
        assert!(r.content.contains("`body` is not a field") && r.content.contains("goes in `text`"), "{}", r.content);
        let r = tool.execute_dyn(&ctx, serde_json::json!({"resource": "mail", "action": "send", "to": "a@example.com", "subject": "Re: quote", "html": "<p>hello</p>"})).await;
        assert!(r.is_error && r.content.contains("`text` is required"), "{}", r.content);
        let r = tool.execute_dyn(&ctx, serde_json::json!({"resource": "mail", "action": "send", "to": "a@example.com", "subject": "Re: quote"})).await;
        assert!(r.is_error && r.content.contains("has no text"), "{}", r.content);
    }

    /// Each call names the current-set tool for the job it does, whichever
    /// shape the model wrote — the key every guard and rule matches.
    #[test]
    fn each_call_keys_on_the_job_it_does() {
        let tool = os();
        for (input, key) in [
            (serde_json::json!({"resource": "capture", "action": "screenshot"}), "desktop_screenshot"),
            (serde_json::json!({"action": "click", "x": 1, "y": 2}), "desktop_click"),
            (serde_json::json!({"resource": "mail", "action": "send", "to": "a@example.com"}), "mail_message_send"),
            (serde_json::json!({"resource": "calendar", "action": "create"}), "calendar_event_create"),
            (serde_json::json!({"resource": "window", "action": "list"}), "window_list"),
        ] {
            assert_eq!(tool.rule_key(&input), key, "{input}");
            assert!(crate::registry::is_tool_name(&tool.rule_key(&input)), "{input}");
        }
    }

    #[test]
    fn reads_are_read_only_and_writes_are_not() {
        let tool = os();
        assert!(tool.read_only(&serde_json::json!({"action": "screenshot"})));
        assert!(tool.read_only(&serde_json::json!({"resource": "search", "action": "search", "query": "x"})));
        assert!(!tool.read_only(&serde_json::json!({"resource": "input", "action": "click", "ref": "B1"})));
        let fx = tool.effects(&serde_json::json!({"resource": "mail", "action": "send", "to": "a@example.com, b@example.com"}));
        assert_eq!(fx.recipients, vec!["a@example.com", "b@example.com"]);
    }

    /// The capability comes from the resource, and mail/calendar sit behind
    /// no toggle.
    #[test]
    fn capability_follows_the_resource() {
        let tool = os();
        let cap = |v: serde_json::Value| tool.capability(&v);
        assert_eq!(cap(serde_json::json!({"resource": "input", "action": "click"})), Some("desktop"));
        assert_eq!(cap(serde_json::json!({"action": "screenshot"})), Some("media"));
        assert_eq!(cap(serde_json::json!({"resource": "settings"})), Some("system"));
        assert_eq!(cap(serde_json::json!({"resource": "contacts", "action": "search"})), Some("contacts"));
        assert_eq!(cap(serde_json::json!({"resource": "mail", "action": "unread"})), None);
    }

    #[test]
    fn test_infer_resource_from_context() {
        // Calendar: date param present → infer "calendar"
        assert_eq!(
            OsTool::infer_resource_from_context(
                &serde_json::json!({"action": "create", "date": "2025-06-15"})
            ),
            "calendar"
        );
        // Reminders: due_date present → infer "reminders"
        assert_eq!(
            OsTool::infer_resource_from_context(
                &serde_json::json!({"action": "create", "due_date": "tomorrow"})
            ),
            "reminders"
        );
        // Contacts: phone present → infer "contacts"
        assert_eq!(
            OsTool::infer_resource_from_context(
                &serde_json::json!({"action": "create", "phone": "555-1234"})
            ),
            "contacts"
        );
        // Mail: to present → infer "mail"
        assert_eq!(
            OsTool::infer_resource_from_context(
                &serde_json::json!({"action": "send", "to": "user@example.com"})
            ),
            "mail"
        );
        // No context → empty
        assert_eq!(
            OsTool::infer_resource_from_context(&serde_json::json!({"action": "create"})),
            ""
        );
    }

    #[test]
    fn test_resolved_resource_mail_read() {
        // "read" with mail params routes to mail
        let input = serde_json::json!({"action": "read", "mailbox": "INBOX", "limit": 5});
        assert_eq!(OsTool::resolved_resource(&input), "mail");
        let input = serde_json::json!({"action": "read", "account": "you@example.com"});
        assert_eq!(OsTool::resolved_resource(&input), "mail");
        // Bare "read" names no resource
        let input = serde_json::json!({"action": "read"});
        assert_eq!(OsTool::resolved_resource(&input), "");
        // Explicit resource always wins
        let input = serde_json::json!({"resource": "mail", "action": "read"});
        assert_eq!(OsTool::resolved_resource(&input), "mail");
    }

    #[test]
    fn test_infer_configure() {
        assert_eq!(OsTool::infer_resource("configure"), "calendar");
    }

    #[test]
    fn test_resource_as_action_autocorrect() {
        // When LLM puts resource name as action (e.g. os(action: "calendar")),
        // the spec should still resolve via inference
        let tool = os();
        // "calendar" as action → infer_resource returns "" → infer_from_context → ""
        // But in execute_dyn, RESOURCE_NAMES check catches it
        let input = serde_json::json!({"action": "calendar"});
        // Should not panic at minimum
        let _ = tool.rule_key(&input);
    }

    #[test]
    fn test_schema_requires_resource() {
        let tool = OsTool::new();
        let schema = tool.schema();
        let required = schema["required"].as_array().unwrap();
        let required_strs: Vec<&str> = required.iter().map(|v| v.as_str().unwrap()).collect();
        // One precise calling convention: `action` is the only required field.
        // `resource` is optional — inferred from the action (read→file), passed
        // only to disambiguate shared actions (create, list). See infer_resource.
        assert!(required_strs.contains(&"action"), "schema must require 'action'");
        assert!(
            !required_strs.contains(&"resource"),
            "resource must NOT be required — it is inferred from action"
        );
    }

    /// A file search asks the engine for its own budget, so the search's own
    /// sentence about narrowing the query is what reaches the model. Every
    /// other resource keeps the runner default.
    #[test]
    fn a_file_search_carries_its_own_execution_budget() {
        let tool = OsTool::new();
        let search = serde_json::json!({"resource": "search", "action": "search", "query": "*.md"});
        let budget = tool
            .execution_timeout(&search)
            .expect("a search declares its own budget");
        assert!(budget < std::time::Duration::from_secs(180), "{budget:?}");
        assert!(
            tool.execution_timeout(&serde_json::json!({"resource": "app", "action": "list"}))
                .is_none(),
            "only search overrides the runner default"
        );
    }

}
