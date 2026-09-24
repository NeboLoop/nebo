//! Compaction: trim old tool results every step; checkpoint the conversation
//! to a boundary row when it nears the window; restore what still matters
//! after the boundary.

pub mod checkpoint;
pub mod restore;
pub mod trim;
