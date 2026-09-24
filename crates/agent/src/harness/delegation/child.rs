//! The ONE constructor of a child turn — single, parallel, DAG and
//! continuation all use it. The child inherits the parent's seat and can
//! only narrow it. WP2.8 moves it here from the children-inherit branch.

use super::super::seat::Seat;
use super::super::{TurnInput, TurnRequest};
use super::HelperSpec;

/// The request for a helper of the parent on `parent_key`.
pub fn child_request(
    _parent: &Seat,
    _parent_key: &str,
    _spec: &HelperSpec,
    _input: TurnInput,
) -> TurnRequest {
    unimplemented!("WP2.8")
}
