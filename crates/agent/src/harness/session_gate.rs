//! Admission: one turn per session; input that arrives while a turn runs is
//! queued as a mid-turn row the running turn hears at its next step. WP2.3
//! moves `admit_turn` and `ActiveTurn` here from `runner.rs`.
