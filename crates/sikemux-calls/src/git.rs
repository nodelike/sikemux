use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct PullRequestUrlParams {
    pub repo: String,
}
