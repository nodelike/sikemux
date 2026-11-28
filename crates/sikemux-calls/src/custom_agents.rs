//! Params of the calls that add and remove the agents the person listed themselves.

use serde::{Deserialize, Serialize};

/// The custom agent to take away, by its id.
#[derive(Serialize, Deserialize)]
pub struct RemoveParams {
    pub kind: String,
}
