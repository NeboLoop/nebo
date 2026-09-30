//! System packages on a cloud bot: the one thing an employee there may do
//! as root.
//!
//! A cloud bot is its own VM holding one customer's data, so installing a
//! package there reaches no one else. The image grants exactly two commands
//! through sudo (`assets/cloud-bot/sudoers`): `apt-get update`, and
//! `apt-get install` of package names. This module reads the same two rules,
//! so the command the safeguard and the shell let through is exactly the
//! command sudo runs. Everywhere else, and for every other command, sudo is
//! refused as before.
//!
//! The VM's system folders are rebuilt on every restart, so what an install
//! added is recorded in the bot's state (`system-packages.json` at the top of
//! the data directory, which the state commit carries), and put back in the
//! background once the server starts (`reinstall`).

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use regex::Regex;
use serde::{Deserialize, Serialize};

/// The image's sudoers rules for the installer.
const SUDOERS: &str = include_str!("../../../assets/cloud-bot/sudoers");

/// The package manager the rules name.
const APT_GET: &str = "/usr/bin/apt-get";

/// Where the record of installed packages lives, in the data directory.
const RECORD: &str = "system-packages.json";

/// The refusal for any other use of sudo on a cloud bot, in the one wording
/// the safeguard and the shell use.
pub const CLOUD_SUDO_REFUSAL: &str = "BLOCKED: on this cloud computer the package installer is the only thing that \
runs as root, as a command of its own: `sudo apt-get update && sudo apt-get install -y <package>`. Any other use of \
sudo, and su, is refused. This is a hard safety limit that cannot be overridden.";

/// The argument patterns the sudoers rules allow for `apt-get`, anchored.
fn rules() -> &'static [Regex] {
    static RULES: OnceLock<Vec<Regex>> = OnceLock::new();
    RULES.get_or_init(|| {
        SUDOERS
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .filter_map(|l| l.split_once("NOPASSWD:").map(|(_, cmnds)| cmnds))
            .flat_map(|cmnds| cmnds.split(", "))
            .filter_map(|cmnd| cmnd.trim().strip_prefix(APT_GET))
            .map(|args| Regex::new(args.trim()).expect("the sudoers installer rule is a valid regex"))
            .collect()
    })
}

/// The packages `command` installs when the command is the package
/// installer and nothing else: one or more `sudo apt-get …` the sudoers
/// rules allow, joined by `&&` or `;`. `None` for anything else, a command
/// that also does something other than install included. `Some` of an empty
/// list for `sudo apt-get update` alone.
pub fn installer(command: &str) -> Option<Vec<String>> {
    let mut packages = Vec::new();
    let mut segments = 0;
    for segment in command.split("&&").flat_map(|s| s.split([';', '\n'])) {
        let words: Vec<&str> = segment.split_whitespace().collect();
        if words.is_empty() {
            continue;
        }
        let ["sudo", "apt-get", args @ ..] = words.as_slice() else {
            return None;
        };
        let args = args.join(" ");
        if !rules().iter().any(|rule| rule.is_match(&args)) {
            return None;
        }
        segments += 1;
        if args.starts_with("install ") {
            packages.extend(args.split(' ').skip(1).filter(|a| !a.starts_with('-')).map(str::to_string));
        }
    }
    (segments > 0).then_some(packages)
}

/// The packages `command` installs when it is the installer (`installer`)
/// and this is a cloud bot (`crate::cloud_bot`), the one place it runs.
pub fn allowed(command: &str) -> Option<Vec<String>> {
    if crate::cloud_bot() { installer(command) } else { None }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Record {
    packages: Vec<String>,
}

fn record_path() -> Option<PathBuf> {
    config::data_dir().ok().map(|d| d.join(RECORD))
}

/// The packages installed on this bot, as recorded, sorted.
pub fn installed() -> Vec<String> {
    record_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<Record>(&b).ok())
        .map(|r| r.packages)
        .unwrap_or_default()
}

/// Add `packages` to the record after an install succeeded.
pub fn record(packages: &[String]) {
    static WRITING: Mutex<()> = Mutex::new(());
    if packages.is_empty() {
        return;
    }
    let Some(path) = record_path() else { return };
    let _guard = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = installed();
    all.extend(packages.iter().cloned());
    all.sort();
    all.dedup();
    let tmp = path.with_extension("json.tmp");
    let written = serde_json::to_vec_pretty(&Record { packages: all })
        .map_err(|e| e.to_string())
        .and_then(|b| std::fs::write(&tmp, b).map_err(|e| e.to_string()))
        .and_then(|_| std::fs::rename(&tmp, &path).map_err(|e| e.to_string()));
    match written {
        Ok(()) => tracing::info!(?packages, "system packages: recorded"),
        Err(e) => tracing::warn!(error = %e, ?packages, "system packages: could not record the install"),
    }
}

/// Where putting the recorded packages back after a restart stands.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Restore {
    /// The reinstall is running now.
    pub running: bool,
    /// Packages it could not put back.
    pub failed: Vec<String>,
}

static RESTORE: Mutex<Restore> = Mutex::new(Restore { running: false, failed: Vec::new() });

/// Where the reinstall after this start stands.
pub fn restore() -> Restore {
    RESTORE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Put the recorded packages back after a restart, on a cloud bot. Runs in
/// the background after the server starts: a boot never waits on the
/// package mirrors, and a failure is logged and shown, never fatal.
pub async fn reinstall() {
    if !crate::cloud_bot() {
        return;
    }
    let packages = installed();
    if packages.is_empty() {
        return;
    }
    RESTORE.lock().unwrap_or_else(|e| e.into_inner()).running = true;
    tracing::info!(?packages, "system packages: putting back what was installed before the restart");
    let failed = match apt(&["update"]).await {
        Err(e) => {
            tracing::warn!(error = %e, "system packages: apt-get update failed; nothing was put back");
            packages.clone()
        }
        Ok(()) => {
            let mut all = vec!["install", "-y", "-q"];
            all.extend(packages.iter().map(String::as_str));
            match apt(&all).await {
                Ok(()) => Vec::new(),
                // One package that no longer installs must not keep the
                // rest out: try each on its own.
                Err(e) => {
                    tracing::warn!(error = %e, "system packages: installing them together failed; trying each");
                    let mut failed = Vec::new();
                    for p in &packages {
                        if let Err(e) = apt(&["install", "-y", "-q", p]).await {
                            tracing::warn!(package = %p, error = %e, "system packages: could not put back");
                            failed.push(p.clone());
                        }
                    }
                    failed
                }
            }
        }
    };
    if failed.is_empty() {
        tracing::info!(count = packages.len(), "system packages: all put back");
    }
    *RESTORE.lock().unwrap_or_else(|e| e.into_inner()) = Restore { running: false, failed };
}

/// Run `sudo -n apt-get <args>`; the last lines of its output on failure.
async fn apt(args: &[&str]) -> Result<(), String> {
    let out = command::new::<tokio::process::Command>("sudo", command::Console::Hidden)
        .arg("-n")
        .arg("apt-get")
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| format!("sudo apt-get could not start: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let tail: Vec<&str> = text.lines().rev().take(5).collect();
    Err(format!("{} ({})", tail.into_iter().rev().collect::<Vec<_>>().join(" | "), out.status))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rules_come_from_the_images_sudoers() {
        assert_eq!(rules().len(), 2, "update and install");
    }

    #[test]
    fn the_installer_alone_is_let_through() {
        for (command, packages) in [
            ("sudo apt-get install -y jq", vec!["jq"]),
            ("sudo apt-get update", vec![]),
            ("sudo apt-get update && sudo apt-get install -y imagemagick g++ libssl-dev", vec!["imagemagick", "g++", "libssl-dev"]),
            ("sudo apt-get update; sudo apt-get install --yes -q --no-install-recommends python3.11-dev", vec!["python3.11-dev"]),
        ] {
            assert_eq!(installer(command), Some(packages.into_iter().map(String::from).collect()), "{command}");
        }
    }

    #[test]
    fn anything_else_through_sudo_is_not() {
        for command in [
            "sudo rm -rf /tmp/x",
            "sudo bash",
            "sudo -s",
            "sudo apt-get -o APT::Update::Pre-Invoke::=/bin/sh update",
            "sudo apt-get install -o APT::Update::Pre-Invoke::=/bin/sh jq",
            "sudo apt-get install -c /tmp/apt.conf jq",
            "sudo apt-get install ./evil.deb",
            "sudo apt-get install /tmp/evil.deb",
            "sudo apt-get install jq-",
            "sudo apt-get remove jq",
            "sudo apt-get update --allow-insecure-repositories",
            "sudo apt-get install -y jq && rm -rf ~",
            "sudo apt-get install -y jq | tee log",
            "sudo apt-get install -y $(cat list)",
            "sudo apt-get install -y `id`",
            "sudo apt-get install -y jq > /etc/passwd",
            "sudo -E apt-get install -y jq",
            "sudo DEBIAN_FRONTEND=noninteractive apt-get install -y jq",
            "sudo /usr/bin/apt-get install -y jq",
            "sudo apt install -y jq",
            "apt-get install -y jq",
            "sudo apt-get install -y JQ",
            "",
        ] {
            assert_eq!(installer(command), None, "{command}");
        }
    }

    #[test]
    fn off_a_cloud_bot_nothing_is_allowed() {
        if crate::cloud_bot() {
            return;
        }
        assert_eq!(allowed("sudo apt-get install -y jq"), None);
    }
}
