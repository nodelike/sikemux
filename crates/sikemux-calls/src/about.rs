use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct NotesParams {
    pub version: String,
}

#[derive(Serialize, Deserialize)]
pub struct AvatarsParams {
    pub urls: Vec<String>,
}

/// The release running here, and whether the person has yet to read its notes.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhatsNew {
    pub version: String,
    /// Shown once the introduction is done, for a release the person has not
    /// marked seen by setting `lastSeenVersion` to it.
    pub unseen: bool,
}

/// The person's shell, and whether git, Homebrew and the plugins' own
/// command-line tools are on the host.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tools {
    pub shell: String,
    pub git: bool,
    pub brew: bool,
    pub aws: bool,
    pub rnd: bool,
}
