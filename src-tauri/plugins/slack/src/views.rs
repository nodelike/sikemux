//! What the Slack pane shows besides a workspace's channels and a channel's messages: a
//! thread, with the text an agent is handed when the person sends it one.

use std::path::Path;

use sikemux_plugin_api::slack::{OpenThread, Thread};

use crate::error::SlackResult;
use crate::slack::{self, ThreadRef, THREAD_MAX};

pub async fn thread_view(data_dir: &Path, input: ThreadRef) -> SlackResult<OpenThread> {
    let thread = slack::thread(data_dir, input).await?;
    let place = slack::place_of(data_dir, &thread.workspace, &thread.channel).await;
    Ok(OpenThread {
        for_agent: for_agent(&thread, &place),
        thread,
    })
}

/// Where the thread is, then each message with who said it and when.
fn for_agent(thread: &Thread, place: &str) -> String {
    let mut text = format!("Slack thread in {place}");
    if let Some(link) = &thread.permalink {
        text.push_str(&format!(" — {link}"));
    }
    for message in &thread.messages {
        text.push_str(&format!(
            "\n\n**{}** ({}):\n{}",
            message.name, message.at, message.text
        ));
        if !message.files.is_empty() {
            text.push_str(&format!("\n  (attached: {})", message.files.join(", ")));
        }
    }
    if thread.truncated {
        text.push_str(&format!(
            "\n\n(The thread is longer; only the first {THREAD_MAX} messages are here.)"
        ));
    }
    if let Some(link) = &thread.permalink {
        text.push_str(&format!(
            "\n\nTo reply in this thread, use slack_post with link {link}."
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use sikemux_plugin_api::slack::Message;

    use super::*;

    fn message(ts: &str, name: &str, text: &str, at: &str, files: &[&str]) -> Message {
        Message {
            ts: ts.into(),
            user: Some("U1".into()),
            name: name.into(),
            bot: false,
            text: text.into(),
            at: at.into(),
            thread_ts: Some("1".into()),
            reply_count: 0,
            files: files.iter().map(|file| file.to_string()).collect(),
        }
    }

    fn thread() -> Thread {
        Thread {
            workspace: "T1".into(),
            channel: "C9".into(),
            ts: "1700000000.123456".into(),
            permalink: Some("https://acme.slack.com/archives/C9/p1700000000123456".into()),
            truncated: false,
            messages: vec![
                message(
                    "1",
                    "Irwan",
                    "Deploy failed on staging",
                    "2026-10-10T09:00:00Z",
                    &["deploy.log"],
                ),
                message("2", "Ankit", "Looking", "2026-10-10T09:02:00Z", &[]),
            ],
        }
    }

    #[test]
    fn says_where_it_is_who_said_what_and_when_and_how_to_reply() {
        let text = for_agent(&thread(), "#deploys");
        assert!(text.starts_with(
            "Slack thread in #deploys — https://acme.slack.com/archives/C9/p1700000000123456"
        ));
        assert!(text.contains(
            "**Irwan** (2026-10-10T09:00:00Z):\nDeploy failed on staging\n  (attached: deploy.log)"
        ));
        assert!(text.contains("**Ankit** (2026-10-10T09:02:00Z):\nLooking"));
        assert!(text.contains(
            "use slack_post with link https://acme.slack.com/archives/C9/p1700000000123456"
        ));
    }

    #[test]
    fn names_the_channel_by_id_when_its_name_is_unknown_and_says_when_the_thread_was_cut() {
        let cut = Thread {
            permalink: None,
            truncated: true,
            ..thread()
        };
        let text = for_agent(&cut, "C9");
        assert!(text.starts_with("Slack thread in C9\n"));
        assert!(text.contains("only the first 1000 messages"));
        assert!(!text.contains("slack_post"));
    }
}
