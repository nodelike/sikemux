//! The suggestion a finished turn ends with: what the agent predicts the person sends next.

/// The message comes between tags, since an agent asked this may answer in its own voice.
pub const QUESTION: &str = "Predict the message the user will most likely send next in this conversation. Write it between <next> and </next>, in the user's own voice and style, in under 20 words. If no next step stands out, write the most useful short message they could send. Reply with the tags and the message, nothing else, and use no tools.";

/// The message in an agent's answer to [`QUESTION`].
pub fn message(answer: &str) -> Option<&str> {
    let (_, rest) = answer.split_once("<next>")?;
    let (message, _) = rest.split_once("</next>")?;
    Some(message.trim()).filter(|message| !message.is_empty())
}
