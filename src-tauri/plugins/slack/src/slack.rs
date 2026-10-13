// What Sikemux reads from and writes to Slack: the channels and direct
// messages a person is in, a channel's recent messages, a thread, a search,
// a post, and who someone is.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::client::Session;
use crate::config;
use crate::error::{SlackError, SlackResult};
use crate::text::{self, MessageLink};

#[derive(Deserialize, Clone, Default)]
pub struct Profile {
    display_name: Option<String>,
    real_name: Option<String>,
    title: Option<String>,
    email: Option<String>,
}

#[derive(Deserialize, Clone)]
pub struct UserRow {
    id: String,
    name: Option<String>,
    real_name: Option<String>,
    tz: Option<String>,
    #[serde(default)]
    is_bot: bool,
    #[serde(default)]
    deleted: bool,
    #[serde(default)]
    profile: Profile,
}

impl UserRow {
    /// The name Slack shows: the display name, else the full name, else the handle.
    fn shown(&self) -> String {
        [
            self.profile.display_name.as_deref(),
            self.profile.real_name.as_deref(),
            self.real_name.as_deref(),
            self.name.as_deref(),
        ]
        .into_iter()
        .flatten()
        .find(|name| !name.trim().is_empty())
        .unwrap_or(&self.id)
        .to_string()
    }
}

/// Names already looked up, per workspace and person, so a thread asks once per person.
static NAMES: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn known(key: &str) -> Option<String> {
    NAMES.lock().ok()?.as_ref()?.get(key).cloned()
}

fn remember(key: String, name: String) {
    if let Ok(mut names) = NAMES.lock() {
        let names = names.get_or_insert_with(HashMap::new);
        if names.len() > 5000 {
            names.clear();
        }
        names.insert(key, name);
    }
}

async fn user_row(session: &Session, id: &str) -> SlackResult<UserRow> {
    let answer = session
        .get("users.info", &[("user", id.to_string())])
        .await?;
    Ok(serde_json::from_value(
        answer.get("user").cloned().unwrap_or(Value::Null),
    )?)
}

/// The names of these people, looking up the ones not known yet. One that cannot be found keeps its id.
async fn names_of(session: &Session, ids: &[String]) -> HashMap<String, String> {
    let team = &session.workspace.id;
    let missing: Vec<&String> = ids
        .iter()
        .filter(|id| known(&format!("{team}/{id}")).is_none())
        .collect();
    let found = futures::future::join_all(missing.iter().map(|id| user_row(session, id))).await;
    for row in found.into_iter().flatten() {
        remember(format!("{team}/{}", row.id), row.shown());
    }
    ids.iter()
        .filter_map(|id| known(&format!("{team}/{id}")).map(|name| (id.clone(), name)))
        .collect()
}

#[derive(Deserialize, Default)]
struct FileRow {
    name: Option<String>,
    title: Option<String>,
}

#[derive(Deserialize)]
pub struct MessageRow {
    ts: String,
    user: Option<String>,
    username: Option<String>,
    bot_id: Option<String>,
    #[serde(default)]
    text: String,
    thread_ts: Option<String>,
    reply_count: Option<u64>,
    #[serde(default)]
    files: Vec<FileRow>,
    subtype: Option<String>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub ts: String,
    pub user: Option<String>,
    pub name: String,
    pub bot: bool,
    /// The text as markdown, people and channels named.
    pub text: String,
    /// When it was written, as an RFC 3339 time.
    pub at: String,
    pub thread_ts: Option<String>,
    pub reply_count: u64,
    pub files: Vec<String>,
}

/// Slack's `1700000000.123456` as `2023-11-14T22:13:20Z`.
pub fn time_of(ts: &str) -> String {
    let secs: i64 = ts
        .split('.')
        .next()
        .and_then(|secs| secs.parse().ok())
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// The calendar date `days` after 1970-01-01, by Howard Hinnant's algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// Messages people wrote, not Slack's own notes such as someone joining.
fn is_said(row: &MessageRow) -> bool {
    !matches!(
        row.subtype.as_deref(),
        Some("channel_join" | "channel_leave" | "channel_topic" | "channel_purpose" | "group_join")
    )
}

pub fn message_of(row: MessageRow, names: &HashMap<String, String>) -> Message {
    let name = row
        .user
        .as_ref()
        .and_then(|id| names.get(id).cloned())
        .or_else(|| row.username.clone())
        .or_else(|| row.user.clone())
        .unwrap_or_else(|| "Slack".into());
    Message {
        at: time_of(&row.ts),
        text: text::to_markdown(&row.text, names),
        bot: row.bot_id.is_some(),
        reply_count: row.reply_count.unwrap_or(0),
        thread_ts: row.thread_ts,
        files: row
            .files
            .into_iter()
            .filter_map(|file| file.title.or(file.name))
            .collect(),
        user: row.user,
        name,
        ts: row.ts,
    }
}

/// The messages with every author and mention named.
async fn messages_of(session: &Session, rows: Vec<MessageRow>) -> Vec<Message> {
    let mut ids: Vec<String> = Vec::new();
    for row in &rows {
        for id in row
            .user
            .iter()
            .cloned()
            .chain(text::mentioned_users(&row.text))
        {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    let names = names_of(session, &ids).await;
    rows.into_iter()
        .filter(is_said)
        .map(|row| message_of(row, &names))
        .collect()
}

#[derive(Deserialize)]
pub struct ChannelRow {
    id: String,
    name: Option<String>,
    #[serde(default)]
    is_im: bool,
    #[serde(default)]
    is_mpim: bool,
    #[serde(default)]
    is_private: bool,
    #[serde(default)]
    is_member: bool,
    user: Option<String>,
    updated: Option<u64>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub id: String,
    pub name: String,
    /// `channel`, `private`, `dm` or `group`.
    pub kind: &'static str,
    pub updated: Option<u64>,
}

#[derive(Deserialize)]
pub struct WorkspaceRef {
    #[serde(default)]
    pub workspace: Option<String>,
}

/// The channels the person is in and their direct messages, channels first, each group by name.
pub async fn channels(data_dir: &Path, input: WorkspaceRef) -> SlackResult<Vec<Channel>> {
    let session = Session::of(data_dir, input.workspace.as_deref()).await?;
    let rows: Vec<ChannelRow> = session
        .get_all(
            "conversations.list",
            "channels",
            &[
                ("types", "public_channel,private_channel,mpim,im".into()),
                ("exclude_archived", "true".into()),
                ("limit", "200".into()),
            ],
            5,
        )
        .await?;
    let dm_people: Vec<String> = rows
        .iter()
        .filter(|row| row.is_im)
        .filter_map(|row| row.user.clone())
        .collect();
    let names = names_of(&session, &dm_people).await;
    let mut channels: Vec<Channel> = rows
        .into_iter()
        .filter(|row| row.is_im || row.is_mpim || row.is_member)
        .map(|row| {
            let kind = if row.is_im {
                "dm"
            } else if row.is_mpim {
                "group"
            } else if row.is_private {
                "private"
            } else {
                "channel"
            };
            let name = match (&row.user, kind) {
                (Some(user), "dm") => names.get(user).cloned().unwrap_or_else(|| user.clone()),
                _ => row.name.clone().unwrap_or_else(|| row.id.clone()),
            };
            Channel {
                id: row.id,
                name,
                kind,
                updated: row.updated,
            }
        })
        .collect();
    channels.sort_by(|a, b| {
        (a.kind == "dm" || a.kind == "group")
            .cmp(&(b.kind == "dm" || b.kind == "group"))
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(channels)
}

fn channel_id(raw: &str) -> SlackResult<String> {
    let id = raw.trim();
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(SlackError::BadArg(format!(
            "`{raw}` is not a Slack channel id"
        )));
    }
    Ok(id.to_string())
}

#[derive(Deserialize)]
pub struct HistoryRef {
    #[serde(default)]
    pub workspace: Option<String>,
    pub channel: String,
    #[serde(default)]
    pub limit: Option<u32>,
}

/// A channel's latest messages, oldest first, each saying how many replies its thread has.
pub async fn history(data_dir: &Path, input: HistoryRef) -> SlackResult<Vec<Message>> {
    let session = Session::of(data_dir, input.workspace.as_deref()).await?;
    let answer = session
        .get(
            "conversations.history",
            &[
                ("channel", channel_id(&input.channel)?),
                ("limit", input.limit.unwrap_or(50).clamp(1, 200).to_string()),
            ],
        )
        .await?;
    let mut rows: Vec<MessageRow> =
        serde_json::from_value(answer.get("messages").cloned().unwrap_or(json!([])))?;
    rows.reverse();
    Ok(messages_of(&session, rows).await)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub workspace: String,
    pub channel: String,
    pub ts: String,
    pub permalink: Option<String>,
    pub messages: Vec<Message>,
    /// The thread is longer than what came back.
    pub truncated: bool,
}

#[derive(Deserialize)]
pub struct ThreadRef {
    #[serde(default)]
    pub workspace: Option<String>,
    /// A link to any message in the thread; or `channel` and `ts` instead.
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub ts: Option<String>,
}

/// The workspace, channel and thread a reference names. A link picks the
/// workspace by its address, so it works whichever workspace is the default.
fn resolve(data_dir: &Path, input: &ThreadRef) -> SlackResult<(Option<String>, String, String)> {
    if let Some(link) = input.link.as_deref().filter(|link| !link.trim().is_empty()) {
        let MessageLink {
            domain,
            channel,
            ts,
            thread_ts,
        } = text::parse_link(link)?;
        let workspace = config::load(data_dir)
            .by_domain(&domain)
            .map(|workspace| workspace.id.clone())
            .ok_or_else(|| {
                SlackError::NotFound(format!(
                    "no workspace at {domain} is signed in here; sign in to it from the Slack pane"
                ))
            })?;
        return Ok((Some(workspace), channel, thread_ts.unwrap_or(ts)));
    }
    match (&input.channel, &input.ts) {
        (Some(channel), Some(ts)) => Ok((
            input.workspace.clone(),
            channel_id(channel)?,
            ts.trim().to_string(),
        )),
        _ => Err(SlackError::BadArg(
            "name a message link, or a channel and ts".into(),
        )),
    }
}

const THREAD_MAX: usize = 1000;

pub async fn thread(data_dir: &Path, input: ThreadRef) -> SlackResult<Thread> {
    let (workspace, channel, ts) = resolve(data_dir, &input)?;
    let session = Session::of(data_dir, workspace.as_deref()).await?;
    let rows: Vec<MessageRow> = session
        .get_all(
            "conversations.replies",
            "messages",
            &[
                ("channel", channel.clone()),
                ("ts", ts.clone()),
                ("limit", "200".into()),
            ],
            5,
        )
        .await?;
    let truncated = rows.len() >= THREAD_MAX;
    let root = rows
        .first()
        .map(|row| row.thread_ts.clone().unwrap_or_else(|| row.ts.clone()))
        .unwrap_or_else(|| ts.clone());
    let permalink = session
        .get(
            "chat.getPermalink",
            &[("channel", channel.clone()), ("message_ts", root.clone())],
        )
        .await
        .ok()
        .and_then(|answer| {
            answer
                .get("permalink")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    Ok(Thread {
        workspace: session.workspace.id.clone(),
        messages: messages_of(&session, rows).await,
        channel,
        ts: root,
        permalink,
        truncated,
    })
}

#[derive(Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    pub workspace: Option<String>,
    pub query: String,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Deserialize)]
struct MatchChannel {
    id: String,
    name: Option<String>,
}

#[derive(Deserialize)]
struct MatchRow {
    ts: String,
    #[serde(default)]
    text: String,
    user: Option<String>,
    username: Option<String>,
    channel: MatchChannel,
    permalink: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub channel: String,
    pub channel_name: Option<String>,
    pub ts: String,
    pub name: String,
    pub text: String,
    pub at: String,
    pub permalink: Option<String>,
}

/// Messages matching Slack's search syntax, newest first: `from:@name in:#channel words`.
pub async fn search(data_dir: &Path, input: SearchQuery) -> SlackResult<Vec<Found>> {
    if input.query.trim().is_empty() {
        return Err(SlackError::BadArg("say what to search for".into()));
    }
    let session = Session::of(data_dir, input.workspace.as_deref()).await?;
    let answer = session
        .get(
            "search.messages",
            &[
                ("query", input.query.trim().to_string()),
                ("count", input.limit.unwrap_or(20).clamp(1, 100).to_string()),
                ("sort", "timestamp".into()),
            ],
        )
        .await?;
    let rows: Vec<MatchRow> = serde_json::from_value(
        answer
            .pointer("/messages/matches")
            .cloned()
            .unwrap_or(json!([])),
    )?;
    let ids: Vec<String> = rows
        .iter()
        .flat_map(|row| {
            row.user
                .iter()
                .cloned()
                .chain(text::mentioned_users(&row.text))
        })
        .collect();
    let names = names_of(&session, &ids).await;
    Ok(rows
        .into_iter()
        .map(|row| Found {
            name: row
                .user
                .as_ref()
                .and_then(|id| names.get(id).cloned())
                .or(row.username)
                .unwrap_or_default(),
            text: text::to_markdown(&row.text, &names),
            at: time_of(&row.ts),
            channel: row.channel.id,
            channel_name: row.channel.name,
            ts: row.ts,
            permalink: row.permalink,
        })
        .collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Post {
    #[serde(default)]
    pub workspace: Option<String>,
    /// A message link: the post goes into that message's thread.
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    pub channel: Option<String>,
    /// With `channel`, the thread to reply in.
    #[serde(default)]
    pub thread_ts: Option<String>,
    pub text: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Posted {
    pub channel: String,
    pub ts: String,
    pub permalink: Option<String>,
}

/// Slack's limit is 40,000 characters; a post this long is better as a file.
const POST_MAX_CHARS: usize = 12_000;

pub async fn post(data_dir: &Path, input: Post) -> SlackResult<Posted> {
    let text = input.text.trim();
    if text.is_empty() {
        return Err(SlackError::BadArg("the message is empty".into()));
    }
    if text.chars().count() > POST_MAX_CHARS {
        return Err(SlackError::BadArg(format!(
            "keep a post under {POST_MAX_CHARS} characters"
        )));
    }
    let (workspace, channel, thread_ts) =
        match input.link.as_deref().filter(|link| !link.trim().is_empty()) {
            Some(_) => {
                let (workspace, channel, ts) = resolve(
                    data_dir,
                    &ThreadRef {
                        workspace: input.workspace.clone(),
                        link: input.link.clone(),
                        channel: None,
                        ts: None,
                    },
                )?;
                (workspace, channel, Some(ts))
            }
            None => {
                let channel = input.channel.as_deref().ok_or_else(|| {
                    SlackError::BadArg("name a channel or a message link to reply to".into())
                })?;
                (
                    input.workspace.clone(),
                    channel_id(channel)?,
                    input.thread_ts.clone(),
                )
            }
        };
    let session = Session::of(data_dir, workspace.as_deref()).await?;
    let mut body = json!({ "channel": channel, "text": text, "unfurl_links": false });
    if let (Some(ts), Some(fields)) = (thread_ts, body.as_object_mut()) {
        fields.insert("thread_ts".into(), json!(ts));
    }
    let answer = session.post("chat.postMessage", &body).await?;
    let ts = answer
        .get("ts")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let permalink = session
        .get(
            "chat.getPermalink",
            &[("channel", channel.clone()), ("message_ts", ts.clone())],
        )
        .await
        .ok()
        .and_then(|answer| {
            answer
                .get("permalink")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    Ok(Posted {
        channel,
        ts,
        permalink,
    })
}

#[derive(Deserialize)]
pub struct PersonQuery {
    #[serde(default)]
    pub workspace: Option<String>,
    /// A user id, an email, or a name to look for.
    pub who: String,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub id: String,
    pub name: String,
    pub handle: Option<String>,
    pub real_name: Option<String>,
    pub title: Option<String>,
    pub email: Option<String>,
    pub time_zone: Option<String>,
    pub bot: bool,
}

fn person_of(row: UserRow) -> Person {
    Person {
        name: row.shown(),
        handle: row.name,
        real_name: row.profile.real_name.or(row.real_name),
        title: row.profile.title.filter(|title| !title.is_empty()),
        email: row.profile.email,
        time_zone: row.tz,
        bot: row.is_bot,
        id: row.id,
    }
}

/// Whether someone's names contain every word asked for.
fn matches_name(row: &UserRow, wanted: &str) -> bool {
    let names = [
        row.name.as_deref(),
        row.real_name.as_deref(),
        row.profile.display_name.as_deref(),
        row.profile.real_name.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
    .to_lowercase();
    wanted
        .split_whitespace()
        .all(|word| names.contains(&word.to_lowercase()))
}

/// Who someone is: by id, by email, or the people whose names match.
pub async fn people(data_dir: &Path, input: PersonQuery) -> SlackResult<Vec<Person>> {
    let session = Session::of(data_dir, input.workspace.as_deref()).await?;
    let who = input.who.trim().trim_start_matches('@');
    if who.is_empty() {
        return Err(SlackError::BadArg("say who to look for".into()));
    }
    let looks_like_id = who.len() >= 8
        && who.starts_with(['U', 'W'])
        && who
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
    if looks_like_id {
        return Ok(vec![person_of(user_row(&session, who).await?)]);
    }
    if who.contains('@') {
        let answer = session
            .get("users.lookupByEmail", &[("email", who.to_string())])
            .await?;
        let row: UserRow =
            serde_json::from_value(answer.get("user").cloned().unwrap_or(Value::Null))?;
        return Ok(vec![person_of(row)]);
    }
    let rows: Vec<UserRow> = session
        .get_all("users.list", "members", &[("limit", "200".into())], 10)
        .await?;
    Ok(rows
        .into_iter()
        .filter(|row| !row.deleted && matches_name(row, who))
        .take(10)
        .map(person_of)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timestamp_reads_as_a_time() {
        assert_eq!(time_of("1700000000.123456"), "2023-11-14T22:13:20Z");
        assert_eq!(time_of("0.000000"), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn a_message_names_its_author_mentions_and_files() {
        let row: MessageRow = serde_json::from_value(json!({
            "ts": "1700000000.123456", "user": "U01", "text": "<@U02> the deploy failed, log attached",
            "thread_ts": "1700000000.123456", "reply_count": 3, "files": [ { "name": "deploy.log", "title": "Deploy log" } ]
        }))
        .expect("message parses");
        let names = HashMap::from([
            ("U01".to_string(), "Irwan".to_string()),
            ("U02".to_string(), "Ankit".to_string()),
        ]);
        let message = message_of(row, &names);
        assert_eq!(
            (message.name.as_str(), message.text.as_str()),
            ("Irwan", "@Ankit the deploy failed, log attached")
        );
        assert_eq!(
            (message.reply_count, message.files.as_slice()),
            (3, ["Deploy log".to_string()].as_slice())
        );
        let bot: MessageRow = serde_json::from_value(
            json!({ "ts": "1.000001", "bot_id": "B1", "username": "CI", "text": "done" }),
        )
        .expect("parses");
        let bot = message_of(bot, &HashMap::new());
        assert_eq!((bot.name.as_str(), bot.bot), ("CI", true));
    }

    #[test]
    fn someone_joining_is_not_a_message() {
        let joined: MessageRow = serde_json::from_value(
            json!({ "ts": "1.000001", "subtype": "channel_join", "text": "joined" }),
        )
        .expect("parses");
        assert!(!is_said(&joined));
        let said: MessageRow =
            serde_json::from_value(json!({ "ts": "1.000001", "text": "hi" })).expect("parses");
        assert!(is_said(&said));
    }

    #[test]
    fn a_person_is_found_by_any_of_their_names() {
        let row: UserRow = serde_json::from_value(json!({
            "id": "U01", "name": "ankit.p", "real_name": "Ankit Patidar", "profile": { "display_name": "Ankit", "title": "Engineer" }
        }))
        .expect("parses");
        assert!(matches_name(&row, "patidar"));
        assert!(matches_name(&row, "Ankit P"));
        assert!(!matches_name(&row, "irwan"));
        let person = person_of(row);
        assert_eq!(
            (person.name.as_str(), person.title.as_deref()),
            ("Ankit", Some("Engineer"))
        );
    }

    #[test]
    fn a_thread_is_named_by_a_link_or_a_channel_and_ts() {
        let dir =
            std::env::temp_dir().join(format!("sikemux-slack-resolve-{}", std::process::id()));
        let mut saved = config::SlackConfig::default();
        saved.upsert(config::Workspace {
            id: "T1".into(),
            name: "Acme".into(),
            domain: "acme.slack.com".into(),
            user_id: "U1".into(),
            user: "ankit".into(),
        });
        config::save(&dir, &saved).expect("saves");
        let by_link = ThreadRef {
            workspace: None,
            link: Some(
                "https://acme.slack.com/archives/C9/p1700000500000200?thread_ts=1700000000.123456"
                    .into(),
            ),
            channel: None,
            ts: None,
        };
        assert_eq!(
            resolve(&dir, &by_link).expect("resolves"),
            (Some("T1".into()), "C9".into(), "1700000000.123456".into())
        );
        let elsewhere = ThreadRef {
            link: Some("https://other.slack.com/archives/C9/p1700000500000200".into()),
            ..by_link
        };
        assert!(resolve(&dir, &elsewhere).is_err());
        let by_ts = ThreadRef {
            workspace: None,
            link: None,
            channel: Some("C9".into()),
            ts: Some("1.000001".into()),
        };
        assert_eq!(resolve(&dir, &by_ts).expect("resolves").1, "C9");
        std::fs::remove_dir_all(dir).ok();
    }
}
