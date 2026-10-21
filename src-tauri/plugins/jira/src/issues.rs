// Reading and changing issues: JQL search, one issue with its comments and the
// transitions it can take, comments, workflow transitions, assignment, new and edited
// issues, links between them, time logged, and the person's saved filters.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sikemux_process::sync::Locked;

use crate::adf;
use crate::auth;
use crate::client::{self, Credentials};
use crate::config::Site;
use crate::error::{JiraError, JiraResult};

const DEFAULT_LIMIT: u64 = 20;
const MAX_LIMIT: u64 = 100;
const MAX_COMMENTS: usize = 50;
const AGENT_COMMENTS: usize = 10;
const AGENT_COMMENT_CHARS: usize = 4000;
const COMMENTS_PER_HIT: usize = 5;
const MAX_CHANGES: usize = 50;
/// Longer values are cut, so a rewritten field does not carry its whole text twice.
const CHANGE_VALUE_CHARS: usize = 200;
pub(crate) const SUMMARY_FIELDS: &str =
    "summary,status,priority,assignee,issuetype,updated,parent,duedate";

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub account_id: String,
    pub name: String,
    pub avatar_url: Option<String>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IssueSummary {
    pub key: String,
    pub summary: String,
    pub status: String,
    /// Jira's three buckets every status belongs to: `new`, `indeterminate` or `done`.
    pub status_category: String,
    pub priority: Option<String>,
    pub assignee: Option<Person>,
    pub issue_type: Option<String>,
    pub sprint: Option<String>,
    pub updated: Option<String>,
    /// The day it is due, as `2026-10-31`.
    pub due: Option<String>,
    /// The epic or other issue it sits under.
    pub parent: Option<Parent>,
    pub url: String,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Parent {
    pub key: String,
    pub summary: String,
    /// Jira's name for the epic's colour, `color_1` to `color_14`, when a board says it.
    pub color: Option<String>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: String,
    pub author: String,
    pub author_avatar_url: Option<String>,
    pub created: String,
    pub body: String,
}

/// One edit to an issue: who made it, when, and the fields it changed.
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub id: String,
    pub author: String,
    pub author_avatar_url: Option<String>,
    pub created: String,
    pub fields: Vec<FieldChange>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FieldChange {
    pub field: String,
    /// Absent when the field was empty, or is long text such as the description.
    pub from: Option<String>,
    pub to: Option<String>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Transition {
    pub id: String,
    pub name: String,
    /// The status the issue is in after taking it.
    pub to: String,
    pub to_category: String,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct IssueDetail {
    #[serde(flatten)]
    pub summary: IssueSummary,
    pub project: Option<String>,
    pub reporter: Option<Person>,
    pub labels: Vec<String>,
    pub created: Option<String>,
    /// The description as markdown.
    pub description: String,
    pub comments: Vec<Comment>,
    /// How many comments there are, which can be more than are shown.
    pub comment_count: u64,
    /// The latest edits, newest first.
    pub history: Vec<Change>,
    pub transitions: Vec<Transition>,
    pub subtasks: Vec<IssueSummary>,
    /// The keys of the issues this one is blocked by.
    pub blocked_by: Vec<String>,
    /// The keys of the issues this one blocks.
    pub blocks: Vec<String>,
}

/// The keys this issue is blocked by and the keys it blocks, from its links.
fn blocking_links(fields: &Value) -> (Vec<String>, Vec<String>) {
    let (mut blocked_by, mut blocks) = (Vec::new(), Vec::new());
    for link in fields
        .get("issuelinks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let named = |key: &str| {
            text(link.pointer(&format!("/type/{key}")))
                .is_some_and(|name| name.eq_ignore_ascii_case("blocks"))
        };
        if !named("name") && !named("outward") {
            continue;
        }
        if let Some(key) = text(link.pointer("/inwardIssue/key")) {
            blocked_by.push(key);
        }
        if let Some(key) = text(link.pointer("/outwardIssue/key")) {
            blocks.push(key);
        }
    }
    (blocked_by, blocks)
}

pub(crate) fn text(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_string)
}

fn person(value: Option<&Value>) -> Option<Person> {
    let value = value.filter(|value| !value.is_null())?;
    Some(Person {
        account_id: text(value.get("accountId")).unwrap_or_default(),
        name: text(value.get("displayName")).unwrap_or_else(|| "someone".into()),
        avatar_url: text(value.pointer("/avatarUrls/48x48")),
    })
}

/// The sprint an issue is in: the active one if several, otherwise the latest.
fn sprint(value: Option<&Value>) -> Option<String> {
    let sprints = value?.as_array()?;
    sprints
        .iter()
        .find(|sprint| sprint.get("state").and_then(Value::as_str) == Some("active"))
        .or_else(|| sprints.last())
        .and_then(|sprint| text(sprint.get("name")))
}

/// An issue as Jira's REST API returns it, reduced to what a list shows.
pub fn summarise(issue: &Value, site: &str, sprint_field: Option<&str>) -> IssueSummary {
    let fields = issue.get("fields").unwrap_or(&Value::Null);
    let key = text(issue.get("key")).unwrap_or_default();
    IssueSummary {
        url: format!("https://{site}/browse/{key}"),
        key,
        summary: text(fields.get("summary")).unwrap_or_default(),
        status: text(fields.pointer("/status/name")).unwrap_or_default(),
        status_category: text(fields.pointer("/status/statusCategory/key")).unwrap_or_default(),
        priority: text(fields.pointer("/priority/name")),
        assignee: person(fields.get("assignee")),
        issue_type: text(fields.pointer("/issuetype/name")),
        sprint: sprint_field.and_then(|field| sprint(fields.get(field))),
        updated: text(fields.get("updated")),
        due: text(fields.get("duedate")),
        parent: fields.get("parent").and_then(|parent| {
            Some(Parent {
                key: text(parent.get("key"))?,
                summary: text(parent.pointer("/fields/summary")).unwrap_or_default(),
                color: None,
            })
        }),
    }
}

pub fn comments(body: &Value) -> Vec<Comment> {
    body.get("comments")
        .and_then(Value::as_array)
        .map(|comments| {
            comments
                .iter()
                .map(|comment| Comment {
                    id: text(comment.get("id")).unwrap_or_default(),
                    author: text(comment.pointer("/author/displayName"))
                        .unwrap_or_else(|| "someone".into()),
                    author_avatar_url: text(comment.pointer("/author/avatarUrls/48x48")),
                    created: text(comment.get("created")).unwrap_or_default(),
                    body: adf::to_markdown(comment.get("body").unwrap_or(&Value::Null)),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The edits in an issue's changelog, as Jira lists them: newest first.
pub fn history(changelog: &Value) -> Vec<Change> {
    changelog
        .get("histories")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_CHANGES)
        .map(|entry| Change {
            id: text(entry.get("id")).unwrap_or_default(),
            author: text(entry.pointer("/author/displayName")).unwrap_or_else(|| "Jira".into()),
            author_avatar_url: text(entry.pointer("/author/avatarUrls/48x48")),
            created: text(entry.get("created")).unwrap_or_default(),
            fields: entry
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|item| {
                    let field = text(item.get("field")).unwrap_or_default();
                    let long = field.eq_ignore_ascii_case("description");
                    let value = |key: &str| {
                        if long {
                            return None;
                        }
                        text(item.get(key))
                            .filter(|value| !value.is_empty())
                            .map(|value| {
                                if value.chars().count() > CHANGE_VALUE_CHARS {
                                    let cut: String =
                                        value.chars().take(CHANGE_VALUE_CHARS).collect();
                                    format!("{cut}…")
                                } else {
                                    value
                                }
                            })
                    };
                    FieldChange {
                        from: value("fromString"),
                        to: value("toString"),
                        field,
                    }
                })
                .collect(),
        })
        .filter(|change| !change.fields.is_empty())
        .collect()
}

/// The newest comments the search returned with an issue, oldest first.
fn latest_comments(issue: &Value) -> Vec<Comment> {
    let mut all = comments(issue.pointer("/fields/comment").unwrap_or(&Value::Null));
    let older = all.len().saturating_sub(COMMENTS_PER_HIT);
    all.split_off(older)
}

pub fn transitions(body: &Value) -> Vec<Transition> {
    body.get("transitions")
        .and_then(Value::as_array)
        .map(|transitions| {
            transitions
                .iter()
                .map(|transition| Transition {
                    id: text(transition.get("id")).unwrap_or_default(),
                    name: text(transition.get("name")).unwrap_or_default(),
                    to: text(transition.pointer("/to/name")).unwrap_or_default(),
                    to_category: text(transition.pointer("/to/statusCategory/key"))
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The transition `wanted` names: by id, by its own name, or by the status it leads to, ignoring case.
pub fn pick<'a>(transitions: &'a [Transition], wanted: &str) -> Option<&'a Transition> {
    let wanted = wanted.trim();
    let same = |a: &str| a.eq_ignore_ascii_case(wanted);
    transitions
        .iter()
        .find(|transition| transition.id == wanted)
        .or_else(|| transitions.iter().find(|transition| same(&transition.name)))
        .or_else(|| transitions.iter().find(|transition| same(&transition.to)))
}

/// The ids of the custom fields Sikemux reads, which differ from site to site.
#[derive(Clone, Default, Debug, PartialEq)]
pub(crate) struct CustomFields {
    pub sprint: Option<String>,
    /// An epic's colour on a board, `ghx-label-1` to `ghx-label-14`.
    pub epic_color: Option<String>,
    /// An issue's own colour by name, such as `teal`, which newer projects keep.
    pub issue_color: Option<String>,
}

impl CustomFields {
    pub(crate) fn from_fields(fields: &Value) -> Self {
        let id_of = |schema: &str| {
            fields.as_array().and_then(|fields| {
                fields
                    .iter()
                    .find(|field| {
                        field.pointer("/schema/custom").and_then(Value::as_str) == Some(schema)
                    })
                    .and_then(|field| text(field.get("id")))
            })
        };
        Self {
            sprint: id_of("com.pyxis.greenhopper.jira:gh-sprint"),
            epic_color: id_of("com.pyxis.greenhopper.jira:gh-epic-color"),
            issue_color: id_of("com.pyxis.greenhopper.jira:jsw-issue-color"),
        }
    }
}

/// Asked once per site.
pub(crate) async fn custom_fields(site: &Site, credentials: &Credentials) -> CustomFields {
    static FIELDS: Mutex<Option<HashMap<String, CustomFields>>> = Mutex::new(None);
    if let Some(known) = FIELDS
        .locked()
        .as_ref()
        .and_then(|fields| fields.get(&site.host).cloned())
    {
        return known;
    }
    let Ok(fields) = client::send(credentials, Method::GET, "/rest/api/3/field", &[], None).await
    else {
        return CustomFields::default();
    };
    let found = CustomFields::from_fields(&fields);
    FIELDS
        .locked()
        .get_or_insert_with(HashMap::new)
        .insert(site.host.clone(), found.clone());
    found
}

pub(crate) async fn sprint_field(site: &Site, credentials: &Credentials) -> Option<String> {
    custom_fields(site, credentials).await.sprint
}

pub(crate) fn issue_path(key: &str) -> JiraResult<String> {
    let key = key.trim();
    let valid = !key.is_empty()
        && key.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        });
    if !valid {
        return Err(JiraError::BadArg(format!(
            "{key:?} is not an issue key like ABC-123"
        )));
    }
    Ok(format!("/rest/api/3/issue/{key}"))
}

#[derive(Deserialize)]
pub struct Search {
    pub jql: String,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub site: Option<String>,
    /// Where the previous page ended, as `next` gave it.
    #[serde(default)]
    pub next: Option<String>,
    #[serde(default)]
    pub comments: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    #[serde(flatten)]
    pub summary: IssueSummary,
    /// The latest few, oldest first; only when the search asked for comments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comments: Option<Vec<Comment>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub issues: Vec<Hit>,
    /// Pass back as `next` for the following page; absent on the last.
    pub next: Option<String>,
}

pub async fn search(data_dir: &Path, request: Search) -> JiraResult<Found> {
    let (site, credentials) = auth::credentials(data_dir, request.site.as_deref()).await?;
    let sprint_field = sprint_field(&site, &credentials).await;
    let mut fields = SUMMARY_FIELDS.to_string();
    if let Some(field) = &sprint_field {
        fields.push(',');
        fields.push_str(field);
    }
    if request.comments {
        fields.push_str(",comment");
    }
    let mut body = json!({
        "jql": request.jql,
        "maxResults": request.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT),
        "fields": fields.split(',').collect::<Vec<_>>(),
    });
    if let (Some(next), Some(object)) = (request.next, body.as_object_mut()) {
        object.insert("nextPageToken".into(), json!(next));
    }
    let answer = client::send(
        &credentials,
        Method::POST,
        "/rest/api/3/search/jql",
        &[],
        Some(&body),
    )
    .await?;
    let issues = answer
        .get("issues")
        .and_then(Value::as_array)
        .map(|issues| {
            issues
                .iter()
                .map(|issue| Hit {
                    summary: summarise(issue, &site.host, sprint_field.as_deref()),
                    comments: request.comments.then(|| latest_comments(issue)),
                })
                .collect()
        })
        .unwrap_or_default();
    let last = answer
        .get("isLast")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    Ok(Found {
        issues,
        next: if last {
            None
        } else {
            text(answer.get("nextPageToken"))
        },
    })
}

#[derive(Deserialize)]
pub struct IssueRequest {
    pub key: String,
    #[serde(default)]
    pub site: Option<String>,
}

#[derive(Deserialize)]
pub struct AgentIssueRequest {
    pub key: String,
    #[serde(default)]
    pub site: Option<String>,
    #[serde(default)]
    pub comments: Option<usize>,
}

/// The issue as an agent reads it: only the newest comments, each cut to a readable length.
pub async fn agent_issue(data_dir: &Path, request: AgentIssueRequest) -> JiraResult<IssueDetail> {
    let keep = request
        .comments
        .unwrap_or(AGENT_COMMENTS)
        .clamp(1, MAX_COMMENTS);
    let mut detail = issue(
        data_dir,
        IssueRequest {
            key: request.key,
            site: request.site,
        },
    )
    .await?;
    let skipped = detail.comments.len().saturating_sub(keep);
    detail.comments.drain(..skipped);
    for comment in &mut detail.comments {
        if let Some((cut, _)) = comment.body.char_indices().nth(AGENT_COMMENT_CHARS) {
            comment.body.truncate(cut);
            comment.body.push('…');
        }
    }
    Ok(detail)
}

pub async fn issue(data_dir: &Path, request: IssueRequest) -> JiraResult<IssueDetail> {
    let (site, credentials) = auth::credentials(data_dir, request.site.as_deref()).await?;
    let path = issue_path(&request.key)?;
    let sprint_field = sprint_field(&site, &credentials).await;
    let mut fields =
        format!("{SUMMARY_FIELDS},project,reporter,labels,created,description,subtasks,issuelinks");
    if let Some(field) = &sprint_field {
        fields.push(',');
        fields.push_str(field);
    }
    let found = client::send(
        &credentials,
        Method::GET,
        &path,
        &[("fields", fields), ("expand", "changelog".into())],
        None,
    )
    .await?;
    let comment_page = client::send(
        &credentials,
        Method::GET,
        &format!("{path}/comment"),
        &[
            ("orderBy", "-created".into()),
            ("maxResults", MAX_COMMENTS.to_string()),
        ],
        None,
    )
    .await?;
    let moves = client::send(
        &credentials,
        Method::GET,
        &format!("{path}/transitions"),
        &[],
        None,
    )
    .await?;
    let all = found.get("fields").unwrap_or(&Value::Null);
    let mut comments = comments(&comment_page);
    comments.reverse();
    let (blocked_by, blocks) = blocking_links(all);
    Ok(IssueDetail {
        summary: summarise(&found, &site.host, sprint_field.as_deref()),
        project: text(all.pointer("/project/key")),
        reporter: person(all.get("reporter")),
        labels: all
            .get("labels")
            .and_then(Value::as_array)
            .map(|labels| {
                labels
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        created: text(all.get("created")),
        description: adf::to_markdown(all.get("description").unwrap_or(&Value::Null)),
        comment_count: comment_page
            .get("total")
            .and_then(Value::as_u64)
            .unwrap_or(comments.len() as u64),
        comments,
        history: history(found.get("changelog").unwrap_or(&Value::Null)),
        transitions: transitions(&moves),
        subtasks: all
            .get("subtasks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|subtask| summarise(subtask, &site.host, None))
            .collect(),
        blocked_by,
        blocks,
    })
}

#[derive(Deserialize)]
pub struct CommentRequest {
    pub key: String,
    pub body: String,
    #[serde(default)]
    pub site: Option<String>,
}

pub async fn comment(data_dir: &Path, request: CommentRequest) -> JiraResult<Comment> {
    if request.body.trim().is_empty() {
        return Err(JiraError::BadArg("the comment is empty".into()));
    }
    let (_, credentials) = auth::credentials(data_dir, request.site.as_deref()).await?;
    let path = format!("{}/comment", issue_path(&request.key)?);
    let posted = client::send(
        &credentials,
        Method::POST,
        &path,
        &[],
        Some(&json!({ "body": adf::from_markdown(&request.body) })),
    )
    .await?;
    Ok(Comment {
        id: text(posted.get("id")).unwrap_or_default(),
        author: text(posted.pointer("/author/displayName")).unwrap_or_default(),
        author_avatar_url: text(posted.pointer("/author/avatarUrls/48x48")),
        created: text(posted.get("created")).unwrap_or_default(),
        body: adf::to_markdown(posted.get("body").unwrap_or(&Value::Null)),
    })
}

#[derive(Deserialize)]
pub struct TransitionRequest {
    pub key: String,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub site: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Moved {
    /// The status now, when a transition was taken.
    pub status: Option<String>,
    /// What the issue can do next.
    pub transitions: Vec<Transition>,
}

/// Takes the transition named, which must be one the issue's workflow offers now; with none named,
/// says which it offers.
pub async fn transition(data_dir: &Path, request: TransitionRequest) -> JiraResult<Moved> {
    let (_, credentials) = auth::credentials(data_dir, request.site.as_deref()).await?;
    let path = format!("{}/transitions", issue_path(&request.key)?);
    let offered = transitions(&client::send(&credentials, Method::GET, &path, &[], None).await?);
    let Some(wanted) = request.to.filter(|wanted| !wanted.trim().is_empty()) else {
        return Ok(Moved {
            status: None,
            transitions: offered,
        });
    };
    let Some(chosen) = pick(&offered, &wanted) else {
        let names = offered
            .iter()
            .map(|transition| format!("{} (→ {})", transition.name, transition.to))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(JiraError::BadArg(format!(
            "{} cannot move to {wanted:?} from where it is now; it can take: {}",
            request.key,
            if names.is_empty() {
                "nothing".into()
            } else {
                names
            }
        )));
    };
    let status = chosen.to.clone();
    client::send(
        &credentials,
        Method::POST,
        &path,
        &[],
        Some(&json!({ "transition": { "id": chosen.id } })),
    )
    .await?;
    let now = transitions(&client::send(&credentials, Method::GET, &path, &[], None).await?);
    Ok(Moved {
        status: Some(status),
        transitions: now,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssignRequest {
    pub key: String,
    /// Whom to assign; absent or empty takes the issue off whoever has it, unless `me` is set.
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub me: bool,
    #[serde(default)]
    pub site: Option<String>,
}

pub async fn assign(data_dir: &Path, request: AssignRequest) -> JiraResult<()> {
    let (site, credentials) = auth::credentials(data_dir, request.site.as_deref()).await?;
    let path = format!("{}/assignee", issue_path(&request.key)?);
    let account = if request.me {
        Some(site.account_id.clone())
    } else {
        request.account_id.filter(|id| !id.is_empty())
    };
    client::send(
        &credentials,
        Method::PUT,
        &path,
        &[],
        Some(&json!({ "accountId": account })),
    )
    .await?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRequest {
    pub key: String,
    /// The task's place among the description's tasks, in reading order.
    pub index: usize,
    /// The task's words as the person saw them, checked before anything is written.
    pub text: String,
    pub done: bool,
    #[serde(default)]
    pub site: Option<String>,
}

/// Ticks or clears one task in an issue's description, writing back Jira's own document so
/// nothing else in it changes.
pub async fn set_task(data_dir: &Path, request: TaskRequest) -> JiraResult<()> {
    let (_, credentials) = auth::credentials(data_dir, request.site.as_deref()).await?;
    let path = issue_path(&request.key)?;
    let issue = client::send(
        &credentials,
        Method::GET,
        &path,
        &[("fields", "description".into())],
        None,
    )
    .await?;
    let mut description = issue
        .get("fields")
        .and_then(|fields| fields.get("description"))
        .cloned()
        .unwrap_or(Value::Null);
    if !adf::set_task(&mut description, request.index, &request.text, request.done) {
        return Err(JiraError::NotFound(format!(
            "{} changed in Jira since it was opened; reload it and try again",
            request.key.trim()
        )));
    }
    client::send(
        &credentials,
        Method::PUT,
        &path,
        &[],
        Some(&json!({ "fields": { "description": description } })),
    )
    .await?;
    Ok(())
}

#[derive(Deserialize)]
pub struct AssignableRequest {
    pub key: String,
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub site: Option<String>,
}

pub async fn assignable(data_dir: &Path, request: AssignableRequest) -> JiraResult<Vec<Person>> {
    let (_, credentials) = auth::credentials(data_dir, request.site.as_deref()).await?;
    issue_path(&request.key)?;
    let people = client::send(
        &credentials,
        Method::GET,
        "/rest/api/3/user/assignable/search",
        &[
            ("issueKey", request.key.trim().to_string()),
            ("query", request.query),
            ("maxResults", "20".into()),
        ],
        None,
    )
    .await?;
    Ok(people
        .as_array()
        .map(|people| {
            people
                .iter()
                .filter_map(|value| person(Some(value)))
                .collect()
        })
        .unwrap_or_default())
}

/// Fields an agent can set when creating or editing an issue. Only those given are sent.
#[derive(Deserialize, Default)]
pub struct Changes {
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub labels: Option<Vec<String>>,
    #[serde(default)]
    pub fields: Map<String, Value>,
}

/// The changes in the shape Jira's `fields` takes. An empty parent or assignee clears it;
/// `me` assigns whoever signed in.
pub fn fields_of(changes: Changes, me: &str) -> JiraResult<Map<String, Value>> {
    let mut fields = Map::new();
    if let Some(summary) = changes.summary {
        if summary.trim().is_empty() {
            return Err(JiraError::BadArg("the summary cannot be empty".into()));
        }
        fields.insert("summary".into(), json!(summary.trim()));
    }
    if let Some(description) = changes.description {
        let document = if description.trim().is_empty() {
            Value::Null
        } else {
            adf::from_markdown(&description)
        };
        fields.insert("description".into(), document);
    }
    if let Some(parent) = changes.parent {
        let parent = parent.trim();
        let value = if parent.is_empty() {
            Value::Null
        } else {
            issue_path(parent)?;
            json!({ "key": parent.to_ascii_uppercase() })
        };
        fields.insert("parent".into(), value);
    }
    if let Some(priority) = changes.priority.filter(|name| !name.trim().is_empty()) {
        fields.insert("priority".into(), json!({ "name": priority.trim() }));
    }
    if let Some(assignee) = changes.assignee {
        let account = match assignee.trim() {
            "" => Value::Null,
            id if id.eq_ignore_ascii_case("me") => json!({ "accountId": me }),
            id => json!({ "accountId": id }),
        };
        fields.insert("assignee".into(), account);
    }
    if let Some(labels) = changes.labels {
        fields.insert("labels".into(), json!(labels));
    }
    fields.extend(changes.fields);
    Ok(fields)
}

#[derive(Deserialize)]
pub struct CreateRequest {
    pub project: String,
    #[serde(default, rename = "type")]
    pub issue_type: Option<String>,
    #[serde(flatten)]
    pub changes: Changes,
    #[serde(default)]
    pub site: Option<String>,
}

#[derive(Serialize)]
pub struct Saved {
    pub key: String,
    pub url: String,
}

fn project_path(project: &str) -> JiraResult<String> {
    let valid = !project.is_empty()
        && project
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_');
    if !valid {
        return Err(JiraError::BadArg(format!(
            "{project:?} is not a project key like ABC"
        )));
    }
    Ok(format!("/rest/api/3/project/{project}"))
}

/// The issue types a project can create, by name.
async fn issue_types(credentials: &Credentials, project: &str) -> JiraResult<Vec<String>> {
    let found = client::send(credentials, Method::GET, &project_path(project)?, &[], None).await?;
    Ok(found
        .get("issueTypes")
        .and_then(Value::as_array)
        .map(|types| {
            types
                .iter()
                .filter_map(|kind| text(kind.get("name")))
                .collect()
        })
        .unwrap_or_default())
}

pub async fn create(data_dir: &Path, request: CreateRequest) -> JiraResult<Saved> {
    let project = request.project.trim().to_ascii_uppercase();
    let has_summary = request
        .changes
        .summary
        .as_deref()
        .is_some_and(|summary| !summary.trim().is_empty());
    if !has_summary || project.is_empty() {
        return Err(JiraError::BadArg(
            "a project key and a summary are both needed".into(),
        ));
    }
    let (site, credentials) = auth::credentials(data_dir, request.site.as_deref()).await?;
    let issue_type = request.issue_type.unwrap_or_else(|| "Task".into());
    let mut fields = fields_of(request.changes, &site.account_id)?;
    fields.insert("project".into(), json!({ "key": project }));
    fields.insert("issuetype".into(), json!({ "name": issue_type }));
    let created = match client::send(
        &credentials,
        Method::POST,
        "/rest/api/3/issue",
        &[],
        Some(&json!({ "fields": fields })),
    )
    .await
    {
        Err(JiraError::Http {
            status: 400,
            message,
        }) if message.to_ascii_lowercase().contains("issuetype") => {
            let types = issue_types(&credentials, &project)
                .await
                .unwrap_or_default();
            return Err(JiraError::BadArg(format!(
                "{message}; {project} has these issue types: {}",
                types.join(", ")
            )));
        }
        outcome => outcome?,
    };
    let key = text(created.get("key"))
        .ok_or_else(|| JiraError::Response("Jira did not say which issue it made".into()))?;
    Ok(Saved {
        url: format!("https://{}/browse/{key}", site.host),
        key,
    })
}

#[derive(Deserialize)]
pub struct EditRequest {
    pub key: String,
    #[serde(flatten)]
    pub changes: Changes,
    #[serde(default)]
    pub site: Option<String>,
}

pub async fn edit(data_dir: &Path, request: EditRequest) -> JiraResult<Saved> {
    let (site, credentials) = auth::credentials(data_dir, request.site.as_deref()).await?;
    let path = issue_path(&request.key)?;
    let fields = fields_of(request.changes, &site.account_id)?;
    if fields.is_empty() {
        return Err(JiraError::BadArg("say which fields to change".into()));
    }
    client::send(
        &credentials,
        Method::PUT,
        &path,
        &[],
        Some(&json!({ "fields": fields })),
    )
    .await?;
    let key = request.key.trim().to_ascii_uppercase();
    Ok(Saved {
        url: format!("https://{}/browse/{key}", site.host),
        key,
    })
}

#[derive(Debug, PartialEq)]
pub struct LinkType {
    pub name: String,
    /// How the issue at the start of the link reads, e.g. "blocks".
    pub outward: String,
    /// How the issue at its end reads, e.g. "is blocked by".
    pub inward: String,
}

pub fn link_types(body: &Value) -> Vec<LinkType> {
    body.get("issueLinkTypes")
        .and_then(Value::as_array)
        .map(|types| {
            types
                .iter()
                .filter_map(|kind| {
                    Some(LinkType {
                        name: text(kind.get("name"))?,
                        outward: text(kind.get("outward"))?,
                        inward: text(kind.get("inward"))?,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The link type `relation` names, and whether the issue it describes starts the link. A type's
/// own name, such as "Blocks", reads as its outward wording.
pub fn relation<'a>(types: &'a [LinkType], relation: &str) -> Option<(&'a LinkType, bool)> {
    let wanted = relation.trim();
    let same = |a: &str| a.eq_ignore_ascii_case(wanted);
    types
        .iter()
        .find(|kind| same(&kind.outward))
        .map(|kind| (kind, true))
        .or_else(|| {
            types
                .iter()
                .find(|kind| same(&kind.inward))
                .map(|kind| (kind, false))
        })
        .or_else(|| {
            types
                .iter()
                .find(|kind| same(&kind.name))
                .map(|kind| (kind, true))
        })
}

/// One of an issue's links, seen from that issue.
struct IssueLink {
    id: String,
    kind: String,
    other: String,
    /// Whether the issue it was read from starts the link, as ABC-1 does in "ABC-1 blocks ABC-2".
    starts: bool,
}

fn links_of(issue: &Value) -> Vec<IssueLink> {
    issue
        .pointer("/fields/issuelinks")
        .and_then(Value::as_array)
        .map(|links| {
            links
                .iter()
                .filter_map(|link| {
                    let outward = text(link.pointer("/outwardIssue/key"));
                    Some(IssueLink {
                        id: text(link.get("id"))?,
                        kind: text(link.pointer("/type/name"))?,
                        starts: outward.is_some(),
                        other: outward.or_else(|| text(link.pointer("/inwardIssue/key")))?,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn links(credentials: &Credentials, path: &str) -> JiraResult<Vec<IssueLink>> {
    let issue = client::send(
        credentials,
        Method::GET,
        path,
        &[("fields", "issuelinks".into())],
        None,
    )
    .await?;
    Ok(links_of(&issue))
}

#[derive(Deserialize)]
pub struct LinkRequest {
    pub key: String,
    pub relation: String,
    pub other: String,
    #[serde(default)]
    pub site: Option<String>,
}

#[derive(Serialize)]
pub struct Linked {
    /// The link as Jira now reads it from `key`, e.g. "ABC-1 blocks ABC-2".
    pub link: String,
}

/// Links two issues so the sentence reads true. Jira's create call is easy to get backwards, so
/// the link is read back from `key` and made again the other way round if it came out reversed.
pub async fn link(data_dir: &Path, request: LinkRequest) -> JiraResult<Linked> {
    let (_, credentials) = auth::credentials(data_dir, request.site.as_deref()).await?;
    let path = issue_path(&request.key)?;
    issue_path(&request.other)?;
    let key = request.key.trim().to_ascii_uppercase();
    let other = request.other.trim().to_ascii_uppercase();
    let types = link_types(
        &client::send(
            &credentials,
            Method::GET,
            "/rest/api/3/issueLinkType",
            &[],
            None,
        )
        .await?,
    );
    let Some((kind, starts)) = relation(&types, &request.relation) else {
        let wordings = types
            .iter()
            .map(|kind| format!("{} / {}", kind.outward, kind.inward))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(JiraError::BadArg(format!(
            "{:?} is not a link this site has; it has: {wordings}",
            request.relation
        )));
    };
    let (start, end) = if starts {
        (&key, &other)
    } else {
        (&other, &key)
    };
    let before: Vec<String> = links(&credentials, &path)
        .await?
        .into_iter()
        .map(|link| link.id)
        .collect();
    let post = |inward: &str, outward: &str| {
        json!({
            "type": { "name": kind.name },
            "inwardIssue": { "key": inward },
            "outwardIssue": { "key": outward },
        })
    };
    client::send(
        &credentials,
        Method::POST,
        "/rest/api/3/issueLink",
        &[],
        Some(&post(start, end)),
    )
    .await?;
    let reversed = links(&credentials, &path).await?.into_iter().find(|link| {
        !before.contains(&link.id)
            && link.kind == kind.name
            && link.other == other
            && link.starts != starts
    });
    if let Some(reversed) = reversed {
        client::send(
            &credentials,
            Method::DELETE,
            &format!("/rest/api/3/issueLink/{}", reversed.id),
            &[],
            None,
        )
        .await?;
        client::send(
            &credentials,
            Method::POST,
            "/rest/api/3/issueLink",
            &[],
            Some(&post(end, start)),
        )
        .await?;
    }
    let wording = if starts { &kind.outward } else { &kind.inward };
    Ok(Linked {
        link: format!("{key} {wording} {other}"),
    })
}

#[derive(Deserialize)]
pub struct WorklogRequest {
    pub key: String,
    pub time: String,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub site: Option<String>,
}

pub async fn worklog(data_dir: &Path, request: WorklogRequest) -> JiraResult<Value> {
    if request.time.trim().is_empty() {
        return Err(JiraError::BadArg("say how long, e.g. 1h 30m".into()));
    }
    let (_, credentials) = auth::credentials(data_dir, request.site.as_deref()).await?;
    let path = format!("{}/worklog", issue_path(&request.key)?);
    let mut body = json!({ "timeSpent": request.time.trim() });
    if let (Some(comment), Some(object)) = (
        request.comment.filter(|text| !text.trim().is_empty()),
        body.as_object_mut(),
    ) {
        object.insert("comment".into(), adf::from_markdown(&comment));
    }
    let logged = client::send(&credentials, Method::POST, &path, &[], Some(&body)).await?;
    Ok(json!({ "id": logged.get("id"), "timeSpent": logged.get("timeSpent") }))
}

#[derive(Deserialize)]
pub struct SiteRequest {
    #[serde(default)]
    pub site: Option<String>,
}

#[derive(Serialize)]
pub struct Filter {
    pub id: String,
    pub name: String,
    pub jql: String,
}

/// The filters the person has starred in Jira, to list beside "assigned to me" and the sprint.
pub async fn filters(data_dir: &Path, request: SiteRequest) -> JiraResult<Vec<Filter>> {
    let (_, credentials) = auth::credentials(data_dir, request.site.as_deref()).await?;
    let found = client::send(
        &credentials,
        Method::GET,
        "/rest/api/3/filter/favourite",
        &[],
        None,
    )
    .await?;
    Ok(found
        .as_array()
        .map(|filters| {
            filters
                .iter()
                .filter_map(|filter| {
                    Some(Filter {
                        id: text(filter.get("id"))?,
                        name: text(filter.get("name"))?,
                        jql: text(filter.get("jql"))?,
                    })
                })
                .collect()
        })
        .unwrap_or_default())
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub key: String,
    pub name: String,
    pub avatar_url: Option<String>,
}

pub fn projects_of(body: &Value) -> Vec<Project> {
    body.get("values")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|project| {
                    Some(Project {
                        key: text(project.get("key"))?,
                        name: text(project.get("name"))?,
                        avatar_url: text(project.pointer("/avatarUrls/48x48")),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The projects the person can browse, most recently busy first.
pub async fn projects(data_dir: &Path, request: SiteRequest) -> JiraResult<Vec<Project>> {
    let (_, credentials) = auth::credentials(data_dir, request.site.as_deref()).await?;
    let found = client::send(
        &credentials,
        Method::GET,
        "/rest/api/3/project/search",
        &[
            ("maxResults", "100".into()),
            ("orderBy", "-lastIssueUpdatedTime".into()),
        ],
        None,
    )
    .await?;
    Ok(projects_of(&found))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_projects_by_key_and_name() {
        let body = json!({ "values": [ { "key": "CIQ", "name": "ChannelIQ" }, { "key": "OPS" } ] });
        assert_eq!(
            projects_of(&body),
            vec![Project {
                key: "CIQ".into(),
                name: "ChannelIQ".into(),
                avatar_url: None,
            }]
        );
    }

    fn sample() -> Value {
        json!({ "key": "ABC-12", "fields": {
            "summary": "Fix the login race",
            "status": { "name": "In Progress", "statusCategory": { "key": "indeterminate" } },
            "priority": { "name": "High" },
            "assignee": { "accountId": "a1", "displayName": "Ana" },
            "issuetype": { "name": "Bug" },
            "updated": "2026-10-04T09:00:00.000+0000",
            "duedate": "2026-10-31",
            "parent": { "key": "ABC-1", "fields": { "summary": "Sign-in" } },
            "customfield_10020": [ { "name": "Sprint 4", "state": "closed" }, { "name": "Sprint 5", "state": "active" } ]
        } })
    }

    #[test]
    fn an_issue_reads_as_what_a_list_shows() {
        assert_eq!(
            summarise(&sample(), "acme.atlassian.net", Some("customfield_10020")),
            IssueSummary {
                key: "ABC-12".into(),
                summary: "Fix the login race".into(),
                status: "In Progress".into(),
                status_category: "indeterminate".into(),
                priority: Some("High".into()),
                assignee: Some(Person {
                    account_id: "a1".into(),
                    name: "Ana".into(),
                    avatar_url: None,
                }),
                issue_type: Some("Bug".into()),
                sprint: Some("Sprint 5".into()),
                updated: Some("2026-10-04T09:00:00.000+0000".into()),
                due: Some("2026-10-31".into()),
                parent: Some(Parent {
                    key: "ABC-1".into(),
                    summary: "Sign-in".into(),
                    color: None,
                }),
                url: "https://acme.atlassian.net/browse/ABC-12".into(),
            }
        );
        let unassigned = json!({ "key": "ABC-1", "fields": { "assignee": null } });
        assert_eq!(
            summarise(&unassigned, "acme.atlassian.net", None).assignee,
            None
        );
    }

    #[test]
    fn a_transition_is_found_by_id_name_or_the_status_it_leads_to() {
        let offered = transitions(&json!({ "transitions": [
            { "id": "21", "name": "Start", "to": { "name": "In Progress" } },
            { "id": "31", "name": "Review", "to": { "name": "In Review" } }
        ] }));
        assert_eq!(
            pick(&offered, "31").map(|transition| transition.name.as_str()),
            Some("Review")
        );
        assert_eq!(
            pick(&offered, "start").map(|transition| transition.id.as_str()),
            Some("21")
        );
        assert_eq!(
            pick(&offered, "in review").map(|transition| transition.id.as_str()),
            Some("31")
        );
        assert_eq!(pick(&offered, "Done"), None);
    }

    #[test]
    fn comments_read_as_markdown_with_their_authors() {
        let page = json!({ "comments": [ { "id": "9", "author": { "displayName": "Ana" }, "created": "2026-10-04",
            "body": { "type": "doc", "content": [ { "type": "paragraph", "content": [ { "type": "text", "text": "Looks good" } ] } ] } } ] });
        assert_eq!(
            comments(&page),
            [Comment {
                id: "9".into(),
                author: "Ana".into(),
                author_avatar_url: None,
                created: "2026-10-04".into(),
                body: "Looks good".into()
            }]
        );
    }

    #[test]
    fn only_issue_keys_go_into_request_paths() {
        assert_eq!(
            issue_path(" ABC-12 ").ok().as_deref(),
            Some("/rest/api/3/issue/ABC-12")
        );
        assert!(issue_path("../../myself").is_err());
        assert!(issue_path("").is_err());
    }
}
