/// What a phone shows. It sends an empty list when it goes behind or locks, since a listed chat
/// stays awake, and calls this only once it draws the chief's Overrule and week lines, since a
/// line shown while the person looks is let go of as seen.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnScreenParams {
    /// The agents whose chats the client shows now, replacing what it showed before.
    pub agent_ids: Vec<String>,
    /// The person is looking at them: the app in front and the phone unlocked.
    pub focused: bool,
}
