use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct NewTerminal {
    #[serde(default)]
    pub project: String,
    /// Opens it among the terminals of no project, in the home folder.
    #[serde(default)]
    pub terminals: bool,
}

impl NewTerminal {
    /// One among the terminals of no project.
    pub fn loose() -> Self {
        Self {
            project: String::new(),
            terminals: true,
        }
    }
}

#[derive(Deserialize, Serialize)]
pub struct SshParams {
    pub host: String,
}

impl SshParams {
    pub fn new(host: &str) -> Self {
        Self {
            host: host.to_owned(),
        }
    }
}
