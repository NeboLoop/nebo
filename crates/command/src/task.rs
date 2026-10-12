//! Windows Task Scheduler: a program Task Scheduler starts, for the current
//! user, in their session. The one way Nebo registers a task: the engine's
//! (started at logon, kept running) and the update helper's (started once).
//!
//! Registered with `schtasks /Create /XML`, so every setting is in one
//! definition ([`xml`]). Each setting is load-bearing (engine service spec
//! §4.2):
//! - priority 5 (normal): Task Scheduler's default 7 runs the program at
//!   below-normal CPU and I/O priority;
//! - no time limit: the default ends a task after 72 hours;
//! - runs on battery, and keeps running when the laptop unplugs;
//! - one instance (`IgnoreNew`): a second start while it runs does nothing;
//! - never wakes the machine, never waits for idle;
//! - "restart on failure" covers a failed *start* only (Task Scheduler does
//!   not start a program again when it exits non-zero).
//!
//! The definition is plain text and tested on every OS; registering one
//! runs `schtasks`, which only Windows has.

use std::process::{Output, Stdio};

/// Who a task runs as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// The user's SID.
    pub sid: String,
    /// A service account (SYSTEM, LOCAL SERVICE, NETWORK SERVICE): it never
    /// logs on, so its tasks run in a session of their own, with no logon
    /// trigger. The house CI runner is one.
    pub service: bool,
}

impl Account {
    pub fn from_sid(sid: &str) -> Self {
        let sid = sid.trim().to_string();
        let service = matches!(sid.as_str(), "S-1-5-18" | "S-1-5-19" | "S-1-5-20");
        Self { sid, service }
    }

    /// The user this process runs as (`whoami /user`).
    pub fn current() -> Result<Self, String> {
        let out = crate::new::<std::process::Command>("whoami", crate::Console::Hidden)
            .args(["/user", "/fo", "csv", "/nh"])
            .output()
            .map_err(|e| format!("whoami: {e}"))?;
        // "domain\user","S-1-5-21-…"
        let line = String::from_utf8_lossy(&out.stdout);
        let sid = line.trim().rsplit(',').next().unwrap_or("").trim_matches('"');
        if !sid.starts_with("S-1-") {
            return Err(format!("whoami gave no SID: {line}"));
        }
        Ok(Self::from_sid(sid))
    }
}

/// One task's program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task<'a> {
    pub description: &'a str,
    /// The program's full path.
    pub command: &'a str,
    /// Its command line after the program, quoted as Windows parses it.
    pub arguments: &'a str,
    /// Started when the user logs on (the engine); otherwise only on demand.
    pub at_logon: bool,
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// One argument on a Windows command line: quoted when it has a space, with
/// no trailing backslash to escape the closing quote.
pub fn quote_arg(s: &str) -> String {
    if s.is_empty() || s.contains([' ', '\t']) { format!("\"{}\"", s.trim_end_matches('\\')) } else { s.to_string() }
}

/// The task's definition, for `account`. Runs in the user's own session
/// ("only when the user is logged on"), never elevated. A service account
/// gets no logon trigger: it never logs on.
pub fn xml(task: &Task, account: &Account) -> String {
    let user = xml_escape(&account.sid);
    let (triggers, principal) = if account.service {
        ("<Triggers />".to_string(), format!("<Principal id=\"Author\"><UserId>{user}</UserId></Principal>"))
    } else {
        (
            if task.at_logon {
                format!("<Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId></LogonTrigger></Triggers>")
            } else {
                "<Triggers />".to_string()
            },
            format!(
                "<Principal id=\"Author\"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal>"
            ),
        )
    };
    let dir = xml_escape(task.command.rsplit_once(['\\', '/']).map_or("", |(dir, _)| dir));
    let description = xml_escape(task.description);
    let command = xml_escape(task.command);
    let arguments = xml_escape(task.arguments);
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Author>NeboAI</Author>
    <Description>{description}</Description>
  </RegistrationInfo>
  {triggers}
  <Principals>{principal}</Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>5</Priority>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>999</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{command}</Command>
      <Arguments>{arguments}</Arguments>
      <WorkingDirectory>{dir}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#
    )
}

/// The text of the first `<tag>…</tag>` inside `<within>…</within>` of a
/// definition, as [`query`] returns it.
pub fn element<'a>(xml: &'a str, within: &str, tag: &str) -> Option<&'a str> {
    let start = xml.find(&format!("<{within}>"))?;
    let end = xml[start..].find(&format!("</{within}>"))? + start;
    let section = &xml[start..end];
    let open = format!("<{tag}>");
    let from = section.find(&open)? + open.len();
    let to = section[from..].find(&format!("</{tag}>"))? + from;
    Some(section[from..to].trim())
}

fn schtasks(args: &[&str]) -> Result<Output, String> {
    crate::new::<std::process::Command>("schtasks", crate::Console::Hidden)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("schtasks: {e}"))
}

fn ok(what: &str, out: Output) -> Result<(), String> {
    if out.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&out.stderr);
    let said = if said.trim().is_empty() { String::from_utf8_lossy(&out.stdout) } else { said };
    Err(format!("{what}: {}", said.trim()))
}

/// Register (or replace) the task `name` (a path like `\NeboAI\Nebo Engine`)
/// for the current user.
pub fn register(name: &str, task: &Task) -> Result<(), String> {
    let definition = xml(task, &Account::current()?);
    // schtasks reads a definition file as UTF-16 with its byte-order mark.
    let file = std::env::temp_dir().join(format!("nebo-task-{}-{}.xml", std::process::id(), name.len()));
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(definition.encode_utf16().flat_map(u16::to_le_bytes));
    std::fs::write(&file, bytes).map_err(|e| format!("{}: {e}", file.display()))?;
    let out = schtasks(&["/Create", "/TN", name, "/XML", &file.to_string_lossy(), "/F"]);
    let _ = std::fs::remove_file(&file);
    ok(&format!("register task {name}"), out?)
}

/// The task's definition as Task Scheduler has it; None when there is none.
pub fn query(name: &str) -> Option<String> {
    let out = schtasks(&["/Query", "/TN", name, "/XML"]).ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Start the task now (a no-op while it runs: `IgnoreNew`).
pub fn run(name: &str) -> Result<(), String> {
    ok(&format!("start task {name}"), schtasks(&["/Run", "/TN", name])?)
}

/// Delete the task; one that is not there is not an error.
pub fn delete(name: &str) -> Result<(), String> {
    if query(name).is_none() {
        return Ok(());
    }
    ok(&format!("delete task {name}"), schtasks(&["/Delete", "/TN", name, "/F"])?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user() -> Account {
        Account::from_sid("S-1-5-21-1-2-3-1001")
    }

    fn engine() -> Task<'static> {
        Task {
            description: "Keeps Nebo working",
            command: r"C:\Users\Owner\AppData\Local\Nebo\nebo.exe",
            arguments: "--engine-supervisor",
            at_logon: true,
        }
    }

    #[test]
    fn a_logon_task_runs_for_the_user_at_normal_priority_with_no_time_limit() {
        let xml = xml(&engine(), &user());
        for setting in [
            "<LogonTrigger><Enabled>true</Enabled><UserId>S-1-5-21-1-2-3-1001</UserId></LogonTrigger>",
            "<LogonType>InteractiveToken</LogonType>",
            "<RunLevel>LeastPrivilege</RunLevel>",
            "<Priority>5</Priority>",
            "<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>",
            "<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>",
            "<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>",
            "<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>",
            "<WakeToRun>false</WakeToRun>",
            "<RunOnlyIfIdle>false</RunOnlyIfIdle>",
            "<StopOnIdleEnd>false</StopOnIdleEnd>",
            "<Interval>PT1M</Interval>",
            r"<Command>C:\Users\Owner\AppData\Local\Nebo\nebo.exe</Command>",
            "<Arguments>--engine-supervisor</Arguments>",
            r"<WorkingDirectory>C:\Users\Owner\AppData\Local\Nebo</WorkingDirectory>",
        ] {
            assert!(xml.contains(setting), "missing {setting}:\n{xml}");
        }
    }

    #[test]
    fn an_on_demand_task_has_no_trigger() {
        let xml = xml(&Task { at_logon: false, ..engine() }, &user());
        assert!(xml.contains("<Triggers />"));
        assert!(xml.contains("<LogonType>InteractiveToken</LogonType>"));
    }

    #[test]
    fn a_service_account_gets_no_logon_trigger() {
        let xml = xml(&engine(), &Account::from_sid("S-1-5-20"));
        assert!(xml.contains("<Triggers />"));
        assert!(xml.contains("<Principal id=\"Author\"><UserId>S-1-5-20</UserId></Principal>"));
        assert!(!xml.contains("LogonTrigger"));
        assert!(!xml.contains("InteractiveToken"));
        assert!(Account::from_sid("S-1-5-18").service);
        assert!(!user().service);
    }

    #[test]
    fn text_is_escaped_and_arguments_quoted() {
        let task = Task { command: r"C:\R&D <x>\nebo.exe", arguments: r#"--home "C:\a b""#, ..engine() };
        let xml = xml(&task, &user());
        assert!(xml.contains(r"<Command>C:\R&amp;D &lt;x&gt;\nebo.exe</Command>"));
        assert!(xml.contains(r"<WorkingDirectory>C:\R&amp;D &lt;x&gt;</WorkingDirectory>"));
        assert!(xml.contains(r"<Arguments>--home &quot;C:\a b&quot;</Arguments>"));
        assert_eq!(quote_arg(r"C:\Run Temp\nebo home\"), r#""C:\Run Temp\nebo home""#);
        assert_eq!(quote_arg(r"C:\x"), r"C:\x");
    }

    #[test]
    fn element_reads_one_section() {
        let xml = xml(&engine(), &user());
        assert_eq!(element(&xml, "Settings", "Enabled"), Some("true"));
        assert_eq!(element(&xml, "Settings", "Priority"), Some("5"));
        assert_eq!(element(&xml, "Exec", "Command"), Some(r"C:\Users\Owner\AppData\Local\Nebo\nebo.exe"));
        // The trigger's `Enabled` is not the task's.
        let off = xml.replace("<Enabled>true</Enabled>\n    <Hidden>", "<Enabled>false</Enabled>\n    <Hidden>");
        assert_eq!(element(&off, "Settings", "Enabled"), Some("false"));
    }
}
