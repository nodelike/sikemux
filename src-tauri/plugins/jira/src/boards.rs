//! Jira Software boards: which ones the person can see, a board's columns with the issues
//! in each, and moving an issue into another column.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_plugin_api::http;

use crate::auth;
use crate::client::{self, Credentials};
use crate::error::{JiraError, JiraResult};
use crate::issues::{
    custom_fields, issue_path, summarise, text, CustomFields, IssueSummary, SUMMARY_FIELDS,
};

/// How many issues one board shows; a board this full is better narrowed in Jira itself.
const BOARD_ISSUES_MAX: usize = 300;
const PAGE: usize = 100;

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    pub id: u64,
    pub name: String,
    /// `scrum` or `kanban`.
    pub kind: String,
    pub project: Option<String>,
    /// The project's key when the board belongs to one rather than to a person.
    pub project_key: Option<String>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Sprint {
    pub id: u64,
    pub name: String,
    pub end: Option<String>,
    pub goal: Option<String>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub name: String,
    pub status_ids: Vec<String>,
    pub issues: Vec<IssueSummary>,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BoardView {
    pub id: u64,
    pub name: String,
    pub kind: String,
    /// The scrum board's sprint in progress; its issues are the board's.
    pub sprint: Option<Sprint>,
    pub columns: Vec<Column>,
    /// More issues matched than the board shows.
    pub truncated: bool,
}

#[derive(Deserialize)]
pub struct BoardsRequest {
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub site: Option<String>,
}

pub fn boards_of(body: &Value) -> Vec<Board> {
    body.get("values")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|board| {
                    Some(Board {
                        id: board.get("id").and_then(Value::as_u64)?,
                        name: text(board.get("name"))?,
                        kind: text(board.get("type")).unwrap_or_else(|| "kanban".into()),
                        project: text(board.pointer("/location/projectKey"))
                            .or_else(|| text(board.pointer("/location/displayName"))),
                        project_key: text(board.pointer("/location/projectKey")),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub async fn boards(data_dir: &Path, request: BoardsRequest) -> JiraResult<Vec<Board>> {
    let (_, credentials) = auth::credentials(data_dir, request.site.as_deref()).await?;
    let mut query = vec![("maxResults", "100".to_string())];
    if !request.query.trim().is_empty() {
        query.push(("name", request.query.trim().to_string()));
    }
    let body = client::send(
        &credentials,
        Method::GET,
        "/rest/agile/1.0/board",
        &query,
        None,
    )
    .await?;
    Ok(boards_of(&body))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BoardRequest {
    pub id: u64,
    #[serde(default)]
    pub site: Option<String>,
    /// Account ids whose cards to show. With none of these set, the whole board shows.
    #[serde(default)]
    pub people: Vec<String>,
    #[serde(default)]
    pub mine: bool,
    #[serde(default)]
    pub unassigned: bool,
}

/// The JQL that keeps only the cards of the people picked on a board, or nothing when nobody is.
pub fn people_clause(request: &BoardRequest) -> Option<String> {
    let mut picks = Vec::new();
    if !request.people.is_empty() {
        let ids: Vec<String> = request
            .people
            .iter()
            .map(|id| format!("\"{}\"", id.replace('\\', "\\\\").replace('"', "\\\"")))
            .collect();
        picks.push(format!("assignee in ({})", ids.join(", ")));
    }
    if request.mine {
        picks.push("assignee = currentUser()".to_string());
    }
    if request.unassigned {
        picks.push("assignee is EMPTY".to_string());
    }
    (!picks.is_empty()).then(|| format!("({})", picks.join(" OR ")))
}

/// A kanban board with a backlog keeps it on its own page, as the first column of its
/// configuration; Jira's board leaves that column off, and so its issues.
pub fn drop_backlog_column(configuration: &mut Value) {
    if let Some(columns) = configuration
        .pointer_mut("/columnConfig/columns")
        .and_then(Value::as_array_mut)
    {
        if !columns.is_empty() {
            columns.remove(0);
        }
    }
}

/// What a kanban board shows: the people picked, the board's own sub-filter, and done work
/// from the last two weeks only, as Jira hides older completed issues.
pub fn kanban_jql(people: Option<&str>, configuration: &Value) -> String {
    let sub_filter = text(configuration.pointer("/subQuery/query"))
        .filter(|query| !query.trim().is_empty())
        .map(|query| format!("({query})"));
    let clauses: Vec<String> = [
        people.map(str::to_string),
        sub_filter,
        Some("(statusCategory != Done OR updated >= -14d)".into()),
    ]
    .into_iter()
    .flatten()
    .collect();
    format!("{} ORDER BY Rank ASC", clauses.join(" AND "))
}

/// The colour an epic wears: its board colour as `color_N` where its project keeps one, else its own colour by name.
pub fn epic_color(fields: &Value, ids: &CustomFields) -> Option<String> {
    let board = ids
        .epic_color
        .as_deref()
        .and_then(|id| text(fields.get(id)))
        .and_then(|label| {
            label
                .strip_prefix("ghx-label-")
                .map(|number| format!("color_{number}"))
        });
    board.or_else(|| {
        ids.issue_color
            .as_deref()
            .and_then(|id| text(fields.get(id)))
    })
}

/// The colours of the epics the cards sit under, read from the epics themselves, by epic key.
async fn parent_colors(
    credentials: &Credentials,
    ids: &CustomFields,
    issues: &[Value],
) -> HashMap<String, String> {
    let wanted: Vec<&str> = [ids.epic_color.as_deref(), ids.issue_color.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    let keys: BTreeSet<String> = issues
        .iter()
        .filter_map(|issue| text(issue.pointer("/fields/parent/key")))
        .collect();
    let mut colors = HashMap::new();
    if wanted.is_empty() {
        return colors;
    }
    for chunk in keys.into_iter().collect::<Vec<_>>().chunks(PAGE) {
        let body = json!({
            "jql": format!("key in ({})", chunk.join(",")),
            "maxResults": chunk.len(),
            "fields": wanted,
        });
        let Ok(answer) = client::send(
            credentials,
            Method::POST,
            "/rest/api/3/search/jql",
            &[],
            Some(&body),
        )
        .await
        else {
            break;
        };
        for epic in answer
            .get("issues")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let fields = epic.get("fields").unwrap_or(&Value::Null);
            if let (Some(key), Some(color)) = (text(epic.get("key")), epic_color(fields, ids)) {
                colors.insert(key, color);
            }
        }
    }
    colors
}

/// Puts each issue in the column that holds its status, keeping the board's column order.
/// Issues whose status no column holds are left off, as Jira's own board does.
pub fn arrange(
    configuration: &Value,
    issues: &[Value],
    site: &str,
    sprint_field: Option<&str>,
    epics: &HashMap<String, String>,
) -> Vec<Column> {
    let mut columns: Vec<Column> = configuration
        .pointer("/columnConfig/columns")
        .and_then(Value::as_array)
        .map(|columns| {
            columns
                .iter()
                .map(|column| Column {
                    name: text(column.get("name")).unwrap_or_default(),
                    status_ids: column
                        .get("statuses")
                        .and_then(Value::as_array)
                        .map(|statuses| {
                            statuses
                                .iter()
                                .filter_map(|status| text(status.get("id")))
                                .collect()
                        })
                        .unwrap_or_default(),
                    issues: Vec::new(),
                })
                .collect()
        })
        .unwrap_or_default();
    for issue in issues {
        let Some(status) = text(issue.pointer("/fields/status/id")) else {
            continue;
        };
        if let Some(column) = columns
            .iter_mut()
            .find(|column| column.status_ids.contains(&status))
        {
            let mut card = summarise(issue, site, sprint_field);
            if let Some(parent) = card.parent.as_mut() {
                parent.color = epics.get(&parent.key).cloned();
            }
            column.issues.push(card);
        }
    }
    columns
}

fn sprint_of(body: &Value) -> Option<Sprint> {
    let sprint = body.get("values")?.as_array()?.first()?;
    Some(Sprint {
        id: sprint.get("id").and_then(Value::as_u64)?,
        name: text(sprint.get("name"))?,
        end: text(sprint.get("endDate")),
        goal: text(sprint.get("goal")).filter(|goal| !goal.trim().is_empty()),
    })
}

/// Reads issues page by page up to the board's limit; says whether more were left.
async fn issues_from(
    credentials: &Credentials,
    path: &str,
    fields: &str,
    jql: Option<&str>,
) -> JiraResult<(Vec<Value>, bool)> {
    let more_left = AtomicBool::new(false);
    let mut found = http::collect_pages(0, BOARD_ISSUES_MAX as u32, |start: usize| {
        let more_left = &more_left;
        async move {
            let mut query = vec![
                ("fields", fields.to_string()),
                ("maxResults", PAGE.to_string()),
                ("startAt", start.to_string()),
            ];
            if let Some(jql) = jql {
                query.push(("jql", jql.to_string()));
            }
            let page = client::send(credentials, Method::GET, path, &query, None).await?;
            let issues = page
                .get("issues")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let total = page.get("total").and_then(Value::as_u64).unwrap_or(0) as usize;
            let next = start + issues.len();
            let more = !issues.is_empty() && next < total;
            more_left.store(more, Ordering::Relaxed);
            Ok::<_, JiraError>((issues, (more && next < BOARD_ISSUES_MAX).then_some(next)))
        }
    })
    .await?;
    found.truncate(BOARD_ISSUES_MAX);
    Ok((found, more_left.into_inner()))
}

pub async fn board(data_dir: &Path, request: BoardRequest) -> JiraResult<BoardView> {
    let (site, credentials) = auth::credentials(data_dir, request.site.as_deref()).await?;
    let base = format!("/rest/agile/1.0/board/{}", request.id);
    let configuration_path = format!("{base}/configuration");
    let backlog_path = format!("{base}/backlog");
    let count_only = [("maxResults", "0".to_string())];
    let (about, configuration, ids, backlog) = tokio::join!(
        client::send(&credentials, Method::GET, &base, &[], None),
        client::send(&credentials, Method::GET, &configuration_path, &[], None),
        custom_fields(&site, &credentials),
        client::send(&credentials, Method::GET, &backlog_path, &count_only, None),
    );
    let sprint_field = ids.sprint.clone();
    let (about, mut configuration) = (about?, configuration?);
    let kind = text(about.get("type")).unwrap_or_else(|| "kanban".into());
    if kind == "kanban" && backlog.is_ok() {
        drop_backlog_column(&mut configuration);
    }
    let sprint = if kind == "scrum" {
        let sprints = client::send(
            &credentials,
            Method::GET,
            &format!("{base}/sprint"),
            &[("state", "active".into())],
            None,
        )
        .await?;
        sprint_of(&sprints)
    } else {
        None
    };
    let fields = match &sprint_field {
        Some(field) => format!("{SUMMARY_FIELDS},{field}"),
        None => SUMMARY_FIELDS.into(),
    };
    let people = people_clause(&request);
    let (issues, truncated) = match &sprint {
        Some(sprint) => {
            issues_from(
                &credentials,
                &format!("{base}/sprint/{}/issue", sprint.id),
                &fields,
                people.as_deref(),
            )
            .await?
        }
        None => {
            let jql = kanban_jql(people.as_deref(), &configuration);
            issues_from(&credentials, &format!("{base}/issue"), &fields, Some(&jql)).await?
        }
    };
    let epics = parent_colors(&credentials, &ids, &issues).await;
    Ok(BoardView {
        id: request.id,
        name: text(about.get("name")).unwrap_or_default(),
        kind,
        columns: arrange(
            &configuration,
            &issues,
            &site.host,
            sprint_field.as_deref(),
            &epics,
        ),
        sprint,
        truncated,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveRequest {
    pub key: String,
    /// The statuses of the column the issue was dropped on.
    pub status_ids: Vec<String>,
    /// The column's name, for the message when the workflow cannot get there.
    #[serde(default)]
    pub column: String,
    #[serde(default)]
    pub site: Option<String>,
}

/// The transition that lands in one of `status_ids`, when the workflow offers one now.
pub fn transition_into(body: &Value, status_ids: &[String]) -> Option<String> {
    body.get("transitions")?
        .as_array()?
        .iter()
        .find_map(|transition| {
            let to = text(transition.pointer("/to/id"))?;
            status_ids
                .contains(&to)
                .then(|| text(transition.get("id")))
                .flatten()
        })
}

/// Moves an issue into a board column by taking the workflow transition that leads there.
pub async fn move_issue(data_dir: &Path, request: MoveRequest) -> JiraResult<()> {
    let (_, credentials) = auth::credentials(data_dir, request.site.as_deref()).await?;
    let path = format!("{}/transitions", issue_path(&request.key)?);
    let offered = client::send(&credentials, Method::GET, &path, &[], None).await?;
    let Some(id) = transition_into(&offered, &request.status_ids) else {
        return Err(JiraError::BadArg(format!(
            "{}'s workflow has no way into {} from where it is now",
            request.key.trim(),
            if request.column.is_empty() {
                "that column"
            } else {
                &request.column
            }
        )));
    };
    client::send(
        &credentials,
        Method::POST,
        &path,
        &[],
        Some(&json!({ "transition": { "id": id } })),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn people_clause_keeps_the_whole_board_when_nobody_is_picked() {
        assert_eq!(people_clause(&BoardRequest::default()), None);
    }

    #[test]
    fn people_clause_matches_anyone_picked() {
        let request = BoardRequest {
            people: vec!["5b10:ac8d".into(), "6c2\"x".into()],
            mine: true,
            unassigned: true,
            ..BoardRequest::default()
        };
        assert_eq!(
            people_clause(&request).as_deref(),
            Some(
                r#"(assignee in ("5b10:ac8d", "6c2\"x") OR assignee = currentUser() OR assignee is EMPTY)"#
            )
        );
    }

    fn issue(key: &str, status: &str) -> Value {
        json!({ "key": key, "fields": {
            "summary": format!("{key} summary"),
            "status": { "id": status, "name": status, "statusCategory": { "key": "new" } }
        } })
    }

    #[test]
    fn lists_boards_with_their_kind_and_project() {
        let body = json!({ "values": [
            { "id": 7, "name": "CIQ board", "type": "scrum", "location": { "projectKey": "CIQ" } },
            { "id": 9, "name": "Support", "type": "kanban", "location": { "displayName": "Help desk" } },
            { "name": "No id" }
        ] });
        assert_eq!(
            boards_of(&body),
            vec![
                Board {
                    id: 7,
                    name: "CIQ board".into(),
                    kind: "scrum".into(),
                    project: Some("CIQ".into()),
                    project_key: Some("CIQ".into()),
                },
                Board {
                    id: 9,
                    name: "Support".into(),
                    kind: "kanban".into(),
                    project: Some("Help desk".into()),
                    project_key: None,
                },
            ]
        );
    }

    #[test]
    fn puts_each_issue_in_the_column_that_holds_its_status() {
        let configuration = json!({ "columnConfig": { "columns": [
            { "name": "To Do", "statuses": [ { "id": "1" } ] },
            { "name": "In Progress", "statuses": [ { "id": "3" }, { "id": "10001" } ] },
            { "name": "Done", "statuses": [ { "id": "10002" } ] }
        ] } });
        let issues = [
            issue("A-1", "3"),
            issue("A-2", "1"),
            issue("A-3", "10001"),
            issue("A-4", "999"),
        ];
        let columns = arrange(
            &configuration,
            &issues,
            "acme.atlassian.net",
            None,
            &HashMap::new(),
        );
        let keys: Vec<(&str, Vec<&str>)> = columns
            .iter()
            .map(|column| {
                (
                    column.name.as_str(),
                    column
                        .issues
                        .iter()
                        .map(|issue| issue.key.as_str())
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            keys,
            [
                ("To Do", vec!["A-2"]),
                ("In Progress", vec!["A-1", "A-3"]),
                ("Done", vec![])
            ]
        );
        assert_eq!(columns[1].status_ids, ["3", "10001"]);
    }

    #[test]
    fn finds_the_transition_into_a_column_or_none() {
        let offered = json!({ "transitions": [
            { "id": "11", "name": "Start", "to": { "id": "3" } },
            { "id": "31", "name": "Done", "to": { "id": "10002" } }
        ] });
        assert_eq!(
            transition_into(&offered, &["10001".into(), "3".into()]),
            Some("11".into())
        );
        assert_eq!(transition_into(&offered, &["1".into()]), None);
    }

    #[test]
    fn reads_the_sprint_in_progress() {
        let body = json!({ "values": [ { "id": 42, "name": "Sprint 12", "endDate": "2026-10-14T10:00:00.000Z", "goal": " " } ] });
        assert_eq!(
            sprint_of(&body),
            Some(Sprint {
                id: 42,
                name: "Sprint 12".into(),
                end: Some("2026-10-14T10:00:00.000Z".into()),
                goal: None
            })
        );
        assert_eq!(sprint_of(&json!({ "values": [] })), None);
    }
}
