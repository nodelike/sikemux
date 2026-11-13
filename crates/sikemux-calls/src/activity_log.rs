use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct CommandUsed {
    pub id: String,
}
