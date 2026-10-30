// Slack's message links, and its message markup read as markdown with people
// and channels named.

use std::collections::HashMap;

use crate::error::{SlackError, SlackResult};

/// A message a Slack link points at.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct MessageLink {
    /// The workspace's address, `acme.slack.com`.
    pub domain: String,
    pub channel: String,
    pub ts: String,
    /// The thread the message is a reply in, when the link says so.
    pub thread_ts: Option<String>,
}

/// `p1700000000123456` as Slack's `1700000000.123456`.
fn ts_of(packed: &str) -> Option<String> {
    let digits = packed.strip_prefix('p')?;
    if digits.len() < 7 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (seconds, micros) = digits.split_at(digits.len() - 6);
    Some(format!("{seconds}.{micros}"))
}

fn plain_ts(ts: &str) -> bool {
    let mut parts = ts.split('.');
    let whole = parts
        .next()
        .is_some_and(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    let fraction = parts
        .next()
        .is_some_and(|part| part.len() == 6 && part.chars().all(|c| c.is_ascii_digit()));
    whole && fraction && parts.next().is_none()
}

/// Reads `https://acme.slack.com/archives/C0123/p1700000000123456?thread_ts=1699999999.000100`.
pub fn parse_link(link: &str) -> SlackResult<MessageLink> {
    let bad = || SlackError::BadArg(format!("`{link}` is not a link to a Slack message"));
    let rest = link.trim().strip_prefix("https://").ok_or_else(bad)?;
    let (domain, path) = rest.split_once('/').ok_or_else(bad)?;
    if !domain.ends_with(".slack.com") || domain.contains('@') {
        return Err(bad());
    }
    let (path, query) = path
        .split_once('?')
        .map_or((path, ""), |(path, query)| (path, query));
    let mut parts = path.trim_end_matches('/').split('/');
    if parts.next() != Some("archives") {
        return Err(bad());
    }
    let channel = parts
        .next()
        .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()))
        .ok_or_else(bad)?;
    let ts = parts.next().and_then(ts_of).ok_or_else(bad)?;
    let thread_ts = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("thread_ts="))
        .filter(|ts| plain_ts(ts))
        .map(str::to_string);
    Ok(MessageLink {
        domain: domain.to_ascii_lowercase(),
        channel: channel.to_string(),
        ts,
        thread_ts,
    })
}

/// The people, channels and user groups a message mentions by id.
pub fn mentioned_users(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for token in text.split('<').skip(1) {
        let Some(inside) = token.split('>').next() else {
            continue;
        };
        if let Some(id) = inside.strip_prefix('@') {
            let id = id.split('|').next().unwrap_or_default();
            if !id.is_empty() && !found.iter().any(|seen: &String| seen == id) {
                found.push(id.to_string());
            }
        }
    }
    found
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Slack's `*bold*`, `_italic_` and `~struck~` as markdown, leaving code alone.
fn emphasis(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (index, part) in text.split('`').enumerate() {
        if index > 0 {
            out.push('`');
        }
        if index % 2 == 1 {
            out.push_str(part);
            continue;
        }
        let mut chars = part.chars().peekable();
        let mut previous: Option<char> = None;
        while let Some(c) = chars.next() {
            let starts_word = previous.is_none_or(|p| p.is_whitespace() || "([{\"'".contains(p));
            let ends_word = |next: Option<&char>| {
                next.is_none_or(|n| n.is_whitespace() || ".,;:!?)]}\"'".contains(*n))
            };
            let marker = match c {
                '*' => Some("**"),
                '~' => Some("~~"),
                '_' => Some("*"),
                _ => None,
            };
            match marker {
                Some(mark) if starts_word && chars.peek().is_some_and(|n| !n.is_whitespace()) => {
                    out.push_str(mark)
                }
                Some(mark)
                    if previous.is_some_and(|p| !p.is_whitespace()) && ends_word(chars.peek()) =>
                {
                    out.push_str(mark)
                }
                _ => out.push(c),
            }
            previous = Some(c);
        }
    }
    out
}

/// A message's text as markdown, with `<@U1>` as `@Name`, `<#C1|general>` as
/// `#general`, `<url|label>` as a link, and Slack's emphasis as markdown's.
pub fn to_markdown(text: &str, names: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((before, after)) = rest.split_once('<') {
        out.push_str(before);
        let Some((inside, tail)) = after.split_once('>') else {
            out.push('<');
            out.push_str(after);
            rest = "";
            break;
        };
        let (target, label) = inside
            .split_once('|')
            .map_or((inside, None), |(target, label)| (target, Some(label)));
        let special = target
            .strip_prefix('!')
            .map(|special| special.split('^').next().unwrap_or_default());
        let shown = if let Some(id) = target.strip_prefix('@') {
            format!(
                "@{}",
                names.get(id).map(String::as_str).or(label).unwrap_or(id)
            )
        } else if let Some(id) = target.strip_prefix('#') {
            format!("#{}", label.unwrap_or(id))
        } else if let Some(special) = special {
            match special {
                "here" | "channel" | "everyone" => format!("@{special}"),
                "subteam" => format!(
                    "@{}",
                    label
                        .map(|label| label.trim_start_matches('@'))
                        .unwrap_or("team")
                ),
                "date" => label.unwrap_or_default().to_string(),
                other => other.to_string(),
            }
        } else if target.starts_with("http://")
            || target.starts_with("https://")
            || target.starts_with("mailto:")
        {
            match label {
                Some(label) if label != target => format!("[{}]({target})", unescape(label)),
                _ => target.to_string(),
            }
        } else {
            format!("<{inside}>")
        };
        out.push_str(&shown);
        rest = tail;
    }
    out.push_str(rest);
    unescape(&emphasis(&out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_message_link_and_the_thread_it_sits_in() -> SlackResult<()> {
        assert_eq!(
            parse_link("https://acme.slack.com/archives/C0123ABC/p1700000000123456")?,
            MessageLink {
                domain: "acme.slack.com".into(),
                channel: "C0123ABC".into(),
                ts: "1700000000.123456".into(),
                thread_ts: None
            }
        );
        let reply = parse_link("https://Acme.slack.com/archives/C0123ABC/p1700000500000200?thread_ts=1700000000.123456&cid=C0123ABC")?;
        assert_eq!(
            (reply.ts.as_str(), reply.thread_ts.as_deref()),
            ("1700000500.000200", Some("1700000000.123456"))
        );
        assert_eq!(reply.domain, "acme.slack.com");
        Ok(())
    }

    #[test]
    fn anything_that_is_not_a_slack_message_link_is_refused() {
        for link in [
            "http://acme.slack.com/archives/C1/p1700000000123456",
            "https://acme.slack.com.evil.test/archives/C1/p1700000000123456",
            "https://acme.slack.com/messages/C1",
            "https://acme.slack.com/archives/C1/p17x",
            "https://acme.slack.com/archives/../p1700000000123456",
            "not a link",
        ] {
            assert!(parse_link(link).is_err(), "{link}");
        }
    }

    #[test]
    fn markup_reads_as_markdown_with_people_and_channels_named() {
        let names = HashMap::from([("U01".to_string(), "Irwan".to_string())]);
        assert_eq!(
            to_markdown("<@U01> can you check <#C9|deploys>? See <https://ci.acme.dev/r/1|the run> &amp; <!here>", &names),
            "@Irwan can you check #deploys? See [the run](https://ci.acme.dev/r/1) & @here"
        );
        assert_eq!(
            to_markdown("<@U02> said *ship it* and _soon_, ~not now~", &names),
            "@U02 said **ship it** and *soon*, ~~not now~~"
        );
        assert_eq!(
            to_markdown("keep `a*b*c` and snake_case_names", &names),
            "keep `a*b*c` and snake_case_names"
        );
        assert_eq!(
            to_markdown("<!subteam^S1|@backend> please", &names),
            "@backend please"
        );
    }

    #[test]
    fn finds_who_a_message_mentions() {
        assert_eq!(
            mentioned_users("<@U01> and <@U02|bo> and <@U01> <#C1|x>"),
            ["U01", "U02"]
        );
    }
}
