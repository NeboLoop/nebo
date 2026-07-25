pub mod bundled;
mod expand;
mod loader;
pub mod manifest;
mod skill;

pub use expand::{SkillContext, build_context, expand_variables};

/// Content hash of a skill file for staged-write conflict detection ('' when
/// unreadable/missing). Shared by the skill tool (stage time) and the
/// learning-approval handler (apply time) — the SAME function on both sides
/// or the comparison is meaningless.
pub fn hash_skill_file(path: &std::path::Path) -> String {
    std::fs::read(path)
        .map(|b| format!("{:016x}", xxhash_rust::xxh3::xxh3_64(&b)))
        .unwrap_or_default()
}
pub use loader::{Loader, resolve_skill_path, write_skill};
pub use skill::{
    Skill, SkillRequirement, SkillSource, SkillSummary, parse_skill_frontmatter, parse_skill_md,
    split_frontmatter, walk_resources_filtered,
};
