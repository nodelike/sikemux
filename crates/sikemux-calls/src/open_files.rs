use serde::{Deserialize, Serialize};

/// The files a connection has open now, by absolute path; none when it shows none.
#[derive(Serialize, Deserialize)]
pub struct ShowFiles {
    pub paths: Vec<String>,
}
