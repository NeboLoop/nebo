use std::collections::HashMap;
use std::path::PathBuf;

/// Runtime context for template variable expansion in skill bodies.
pub struct SkillContext {
    /// Directory containing the SKILL.md file.
    pub skill_dir: String,
    /// Persistent data directory for this skill (`<data_dir>/nebo/skills/<name>/data/`).
    pub data_dir: String,
    /// User's configured display name.
    pub user_name: String,
    /// Mapped OS name: `macos`, `linux`, or `windows`.
    pub os: String,
    /// CPU architecture from `std::env::consts::ARCH`.
    pub arch: String,
    /// Resolved plugin binary paths keyed by slug (e.g., "gws" -> "/path/to/gws").
    pub plugin_bins: HashMap<String, String>,
    /// Decrypted secret values keyed by secret key (e.g., "BRAVE_API_KEY" -> "...").
    pub secrets: HashMap<String, String>,
}

/// Replace `${VAR}` template variables in a skill body with runtime values.
///
/// Supported variables:
/// - `${NEBO_SKILL_DIR}` — directory containing the SKILL.md
/// - `${NEBO_DATA_DIR}` — persistent data directory for this skill (created lazily)
/// - `${NEBO_USER_NAME}` — user's configured name
/// - `${NEBO_OS}` — `macos`, `linux`, or `windows`
/// - `${NEBO_ARCH}` — CPU architecture (e.g., `aarch64`, `x86_64`)
/// - `${plugin.SLUG_BIN}` — resolved binary path for a plugin dependency
/// - `${secret.KEY}` — decrypted secret value
pub fn expand_variables(body: &str, ctx: &SkillContext) -> String {
    // Fast path: no variables to expand
    if !body.contains("${") {
        return body.to_string();
    }

    let mut result = body.to_string();

    // Static variables
    result = result.replace("${NEBO_SKILL_DIR}", &ctx.skill_dir);
    result = result.replace("${NEBO_USER_NAME}", &ctx.user_name);
    result = result.replace("${NEBO_OS}", &ctx.os);
    result = result.replace("${NEBO_ARCH}", &ctx.arch);

    // Data dir: create lazily only if the variable is actually referenced
    if result.contains("${NEBO_DATA_DIR}") {
        let data_path = PathBuf::from(&ctx.data_dir);
        if !data_path.exists() {
            if let Err(e) = std::fs::create_dir_all(&data_path) {
                tracing::warn!(
                    path = %data_path.display(),
                    error = %e,
                    "failed to create skill data directory"
                );
            }
        }
        result = result.replace("${NEBO_DATA_DIR}", &ctx.data_dir);
    }

    // Plugin binary paths: ${plugin.SLUG_BIN}
    for (slug, bin_path) in &ctx.plugin_bins {
        let var_name = format!(
            "${{plugin.{}_BIN}}",
            slug.to_uppercase().replace('-', "_")
        );
        result = result.replace(&var_name, bin_path);
    }

    // Secrets: ${secret.KEY}
    for (key, value) in &ctx.secrets {
        let var_name = format!("${{secret.{}}}", key);
        result = result.replace(&var_name, value);
    }

    result
}

/// Build a `SkillContext` for a given skill at activation time.
///
/// Resolves the skill directory, data directory, user name, platform info,
/// plugin binary paths, and decrypted secrets.
pub fn build_context(
    skill: &super::Skill,
    plugin_store: Option<&napp::plugin::PluginStore>,
    store: Option<&db::Store>,
) -> SkillContext {
    // Skill directory: parent of SKILL.md
    let skill_dir = skill
        .base_dir
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    // Persistent data directory: <data_dir>/nebo/skills/<name>/data/
    let data_dir = config::data_dir()
        .map(|d| {
            d.join("nebo")
                .join("skills")
                .join(&skill.name)
                .join("data")
                .to_string_lossy()
                .to_string()
        })
        .unwrap_or_default();

    // User name from profile
    let user_name = store
        .and_then(|s| s.get_user_profile().ok().flatten())
        .and_then(|p| p.display_name)
        .unwrap_or_else(|| "User".to_string());

    // OS mapping
    let os = match std::env::consts::OS {
        "macos" => "macos",
        "linux" => "linux",
        "windows" => "windows",
        other => other,
    }
    .to_string();

    let arch = std::env::consts::ARCH.to_string();

    // Resolve plugin binary paths
    let mut plugin_bins = HashMap::new();
    if let Some(ps) = plugin_store {
        for dep in &skill.plugins {
            if let Some(bin_path) = ps.resolve(&dep.name, &dep.version) {
                plugin_bins.insert(
                    dep.name.clone(),
                    bin_path.to_string_lossy().to_string(),
                );
            }
        }
    }

    // Resolve secrets
    let mut secrets = HashMap::new();
    if let Some(s) = store {
        for decl in skill.secrets() {
            if let Ok(Some(encrypted)) = s.get_skill_secret(&skill.name, &decl.key) {
                if let Ok(plaintext) = auth::credential::decrypt(&encrypted) {
                    secrets.insert(decl.key.clone(), plaintext);
                }
            }
        }
    }

    SkillContext {
        skill_dir,
        data_dir,
        user_name,
        os,
        arch,
        plugin_bins,
        secrets,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_no_variables() {
        let ctx = SkillContext {
            skill_dir: "/skills/test".into(),
            data_dir: "/data/test".into(),
            user_name: "Alice".into(),
            os: "macos".into(),
            arch: "aarch64".into(),
            plugin_bins: HashMap::new(),
            secrets: HashMap::new(),
        };
        let body = "This is plain text with no variables.";
        assert_eq!(expand_variables(body, &ctx), body);
    }

    #[test]
    fn test_expand_static_variables() {
        let ctx = SkillContext {
            skill_dir: "/home/user/skills/research".into(),
            data_dir: "/data/nebo/skills/research/data".into(),
            user_name: "Alice".into(),
            os: "macos".into(),
            arch: "aarch64".into(),
            plugin_bins: HashMap::new(),
            secrets: HashMap::new(),
        };
        let body = "Skill dir: ${NEBO_SKILL_DIR}\nUser: ${NEBO_USER_NAME}\nOS: ${NEBO_OS}\nArch: ${NEBO_ARCH}";
        let expanded = expand_variables(body, &ctx);
        assert_eq!(
            expanded,
            "Skill dir: /home/user/skills/research\nUser: Alice\nOS: macos\nArch: aarch64"
        );
    }

    #[test]
    fn test_expand_data_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let data_path = tmp.path().join("skill_data");

        let ctx = SkillContext {
            skill_dir: String::new(),
            data_dir: data_path.to_string_lossy().to_string(),
            user_name: "User".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            plugin_bins: HashMap::new(),
            secrets: HashMap::new(),
        };

        let body = "Store files in ${NEBO_DATA_DIR}/output.json";
        let expanded = expand_variables(body, &ctx);
        assert!(expanded.contains("/skill_data/output.json"));
        // Directory should be created lazily
        assert!(data_path.exists());
    }

    #[test]
    fn test_expand_data_dir_not_created_when_unused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let data_path = tmp.path().join("should_not_exist");

        let ctx = SkillContext {
            skill_dir: String::new(),
            data_dir: data_path.to_string_lossy().to_string(),
            user_name: "User".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            plugin_bins: HashMap::new(),
            secrets: HashMap::new(),
        };

        let body = "No data dir reference here, just ${NEBO_OS}.";
        let expanded = expand_variables(body, &ctx);
        assert_eq!(expanded, "No data dir reference here, just linux.");
        // Directory should NOT be created since it wasn't referenced
        assert!(!data_path.exists());
    }

    #[test]
    fn test_expand_plugin_bins() {
        let mut plugin_bins = HashMap::new();
        plugin_bins.insert("gws".to_string(), "/plugins/gws/1.2.0/gws".to_string());
        plugin_bins.insert(
            "my-tool".to_string(),
            "/plugins/my-tool/2.0.0/my-tool".to_string(),
        );

        let ctx = SkillContext {
            skill_dir: String::new(),
            data_dir: String::new(),
            user_name: "User".into(),
            os: "macos".into(),
            arch: "aarch64".into(),
            plugin_bins,
            secrets: HashMap::new(),
        };

        let body = "Run ${plugin.GWS_BIN} --help\nOr use ${plugin.MY_TOOL_BIN} process";
        let expanded = expand_variables(body, &ctx);
        assert_eq!(
            expanded,
            "Run /plugins/gws/1.2.0/gws --help\nOr use /plugins/my-tool/2.0.0/my-tool process"
        );
    }

    #[test]
    fn test_expand_secrets() {
        let mut secrets = HashMap::new();
        secrets.insert("BRAVE_API_KEY".to_string(), "BSA-12345".to_string());

        let ctx = SkillContext {
            skill_dir: String::new(),
            data_dir: String::new(),
            user_name: "User".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            plugin_bins: HashMap::new(),
            secrets,
        };

        let body = "API key: ${secret.BRAVE_API_KEY}";
        let expanded = expand_variables(body, &ctx);
        assert_eq!(expanded, "API key: BSA-12345");
    }

    #[test]
    fn test_expand_mixed() {
        let mut plugin_bins = HashMap::new();
        plugin_bins.insert("gws".to_string(), "/bin/gws".to_string());

        let mut secrets = HashMap::new();
        secrets.insert("TOKEN".to_string(), "abc123".to_string());

        let ctx = SkillContext {
            skill_dir: "/skills/test".into(),
            data_dir: "/data/test".into(),
            user_name: "Bob".into(),
            os: "macos".into(),
            arch: "aarch64".into(),
            plugin_bins,
            secrets,
        };

        let body = "Hello ${NEBO_USER_NAME}! Run ${plugin.GWS_BIN} with token ${secret.TOKEN} on ${NEBO_OS}/${NEBO_ARCH}";
        let expanded = expand_variables(body, &ctx);
        assert_eq!(
            expanded,
            "Hello Bob! Run /bin/gws with token abc123 on macos/aarch64"
        );
    }

    #[test]
    fn test_expand_unrecognized_variables_preserved() {
        let ctx = SkillContext {
            skill_dir: String::new(),
            data_dir: String::new(),
            user_name: "User".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            plugin_bins: HashMap::new(),
            secrets: HashMap::new(),
        };

        let body = "Known: ${NEBO_OS}, Unknown: ${SOME_OTHER_VAR}";
        let expanded = expand_variables(body, &ctx);
        assert_eq!(expanded, "Known: linux, Unknown: ${SOME_OTHER_VAR}");
    }
}
