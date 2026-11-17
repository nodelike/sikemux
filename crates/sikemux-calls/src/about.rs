use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct NotesParams {
    pub version: String,
    /// The build whose notes the person last read: the notes then list every change since it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
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
    /// For a nightly whose notes wait unread, the build the person last read. A nightly never
    /// opens its notes by itself; the rail offers them quietly instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
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
