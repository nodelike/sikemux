// Reading and changing issues: JQL search, one issue with its comments and the
// transitions it can take, comments, workflow transitions, assignment, new
// issues, time logged, and the person's saved filters.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::adf;
use crate::auth;
use crate::client::{self, Credentials};
use crate::config::Site;
use crate::error::{JiraError, JiraResult};

const DEFAULT_LIMIT: u64 = 20;
const MAX_LIMIT: u64 = 100;
const MAX_COMMENTS: usize = 50;
pub(crate) const SUMMARY_FIELDS: &str = "summary,status,priority,assignee,issuetype,updated";

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub account_id: String,
    pub name: String,
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
    pub url: String,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: String,
    pub author: String,
    pub created: String,
    pub body: String,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Transition {
    pub id: String,
    pub name: String,
    /// The status the issue is in after taking it.
    pub to: String,
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
    pub transitions: Vec<Transition>,
}

pub(crate) fn text(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_string)
}

fn person(value: Option<&Value>) -> Option<Person> {
    let value = value.filter(|value| !value.is_null())?;
    Some(Person {
        account_id: text(value.get("accountId")).unwrap_or_default(),
        name: text(value.get("displayName")).unwrap_or_else(|| "someone".into()),
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
                    created: text(comment.get("created")).unwrap_or_default(),
                    body: adf::to_markdown(comment.get("body").unwrap_or(&Value::Null)),
                })
                .collect()
        })
        .unwrap_or_default()
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

/// Issue keys such as `ABC-123` in a branch name or commit message, upper-cased, in order and
/// without repeats. A project key is 2 to 10 letters starting a word, as Jira makes them.
pub fn keys_in(text: &str) -> Vec<String> {
    let characters: Vec<char> = text.chars().collect();
    let at = |index: usize| characters.get(index).copied();
    let mut found: Vec<String> = Vec::new();
    let mut index = 0;
    while index < characters.len() {
        let starts_word =
            index == 0 || at(index - 1).is_some_and(|before| !before.is_ascii_alphanumeric());
        let letters = characters
            .iter()
            .skip(index)
            .take_while(|character| character.is_ascii_alphabetic())
            .count();
        if starts_word && (2..=10).contains(&letters) && at(index + letters) == Some('-') {
            let digits = characters
                .iter()
                .skip(index + letters + 1)
                .take_while(|character| character.is_ascii_digit())
                .count();
            let end = index + letters + 1 + digits;
            if digits > 0 && at(end).is_none_or(|next| !next.is_ascii_alphanumeric()) {
                let project: String = characters.iter().skip(index).take(letters).collect();
                let number: String = characters
                    .iter()
                    .skip(index + letters + 1)
                    .take(digits)
                    .collect();
                let key = format!("{}-{number}", project.to_ascii_uppercase());
                if !found.contains(&key) {
                    found.push(key);
                }
                index = end;
                continue;
            }
        }
        index += letters.max(1);
    }
    found
}

/// The custom field that holds sprints, which differs from site to site. Asked once per site.
pub(crate) async fn sprint_field(site: &Site, credentials: &Credentials) -> Option<String> {
    static FIELDS: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);
    if let Some(known) = FIELDS.lock().ok().and_then(|fields| {
        fields
            .as_ref()
            .and_then(|fields| fields.get(&site.host).cloned())
    }) {
        return known;
    }
    let found = client::send(credentials, Method::GET, "/rest/api/3/field", &[], None)
        .await
        .ok()
        .and_then(|fields| {
            fields.as_array().and_then(|fields| {
                fields
                    .iter()
                    .find(|field| {
                        field.pointer("/schema/custom").and_then(Value::as_str)
                            == Some("com.pyxis.greenhopper.jira:gh-sprint")
                    })
                    .and_then(|field| text(field.get("id")))
            })
        });
    if let Ok(mut fields) = FIELDS.lock() {
        fields
            .get_or_insert_with(HashMap::new)
            .insert(site.host.clone(), found.clone());
    }
    found
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
pub struct KeysRequest {
    pub text: String,
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
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub issues: Vec<IssueSummary>,
    /// Pass back as `next` for the following page; absent on the last.
    pub next: Option<String>,
}

pub async fn search(data_dir: &Path, request: Search) -> JiraResult<Found> {
    let (site, credentials) = auth::credentials(data_dir, request.site.as_deref()).await?;
    let sprint_field = sprint_field(&site, &credentials).await;
    let fields = match &sprint_field {
        Some(field) => format!("{SUMMARY_FIELDS},{field}"),
        None => SUMMARY_FIELDS.into(),
    };
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
                .map(|issue| summarise(issue, &site.host, sprint_field.as_deref()))
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

pub async fn issue(data_dir: &Path, request: IssueRequest) -> JiraResult<IssueDetail> {
    let (site, credentials) = auth::credentials(data_dir, request.site.as_deref()).await?;
    let path = issue_path(&request.key)?;
    let sprint_field = sprint_field(&site, &credentials).await;
    let mut fields = format!("{SUMMARY_FIELDS},project,reporter,labels,created,description");
    if let Some(field) = &sprint_field {
        fields.push(',');
        fields.push_str(field);
    }
    let found = client::send(
        &credentials,
        Method::GET,
        &path,
        &[("fields", fields)],
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
        transitions: transitions(&moves),
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

#[derive(Deserialize)]
pub struct CreateRequest {
    pub project: String,
    pub summary: String,
    #[serde(default, rename = "type")]
    pub issue_type: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub site: Option<String>,
}

#[derive(Serialize)]
pub struct Created {
    pub key: String,
    pub url: String,
}

pub async fn create(data_dir: &Path, request: CreateRequest) -> JiraResult<Created> {
    if request.summary.trim().is_empty() || request.project.trim().is_empty() {
        return Err(JiraError::BadArg(
            "a project key and a summary are both needed".into(),
        ));
    }
    let (site, credentials) = auth::credentials(data_dir, request.site.as_deref()).await?;
    let mut fields = json!({
        "project": { "key": request.project.trim().to_ascii_uppercase() },
        "summary": request.summary.trim(),
        "issuetype": { "name": request.issue_type.unwrap_or_else(|| "Task".into()) },
    });
    if let (Some(description), Some(object)) = (
        request.description.filter(|text| !text.trim().is_empty()),
        fields.as_object_mut(),
    ) {
        object.insert("description".into(), adf::from_markdown(&description));
    }
    let created = client::send(
        &credentials,
        Method::POST,
        "/rest/api/3/issue",
        &[],
        Some(&json!({ "fields": fields })),
    )
    .await?;
    let key = text(created.get("key"))
        .ok_or_else(|| JiraError::Response("Jira did not say which issue it made".into()))?;
    Ok(Created {
        url: format!("https://{}/browse/{key}", site.host),
        key,
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
pub struct Project {
    pub key: String,
    pub name: String,
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
                name: "ChannelIQ".into()
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
                    name: "Ana".into()
                }),
                issue_type: Some("Bug".into()),
                sprint: Some("Sprint 5".into()),
                updated: Some("2026-10-04T09:00:00.000+0000".into()),
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
                created: "2026-10-04".into(),
                body: "Looks good".into()
            }]
        );
    }

    #[test]
    fn issue_keys_are_found_in_branch_names_and_commit_messages() {
        assert_eq!(keys_in("abc-123-fix-x"), ["ABC-123"]);
        assert_eq!(
            keys_in("feature/PROJ-7_and_ABC-12: tidy, see ABC-12"),
            ["PROJ-7", "ABC-12"]
        );
        assert_eq!(keys_in("main"), Vec::<String>::new());
        assert_eq!(keys_in("v2-1 and x-1 and ABC-1x"), Vec::<String>::new());
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
