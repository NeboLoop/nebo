//! Old tool call shapes → the current tool set, for the one-time upgrade
//! migration that rewrites stored workflows, employee manifests, installed
//! skills, grants and scheduled jobs. Never consulted at runtime: nothing
//! aliases an old name to a running tool. Each tool package adds the rows
//! for the shapes it retires.

/// One retired call shape and the tool that does its job now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rename {
    pub tool: &'static str,
    /// The old `resource`, when the shape had one.
    pub resource: Option<&'static str>,
    /// The old `action`, when the shape had one.
    pub action: Option<&'static str>,
    pub to: &'static str,
    /// Parameters renamed on the way: (old, new).
    pub params: &'static [(&'static str, &'static str)],
}

pub const RENAMES: &[Rename] = &[Rename {
    tool: "tool_search",
    resource: None,
    action: None,
    to: crate::find_tools::FIND_TOOLS,
    params: &[],
}];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_points_at_a_current_name_and_old_names_are_unique_per_shape() {
        for (i, r) in RENAMES.iter().enumerate() {
            assert!(crate::registry::is_tool_name(r.to), "{} is not a current tool name", r.to);
            assert!(
                RENAMES[..i]
                    .iter()
                    .all(|o| (o.tool, o.resource, o.action) != (r.tool, r.resource, r.action)),
                "duplicate shape {r:?}"
            );
        }
    }
}
