use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
pub struct CommitParams {
    pub dir: PathBuf,
    pub message: String,
}
