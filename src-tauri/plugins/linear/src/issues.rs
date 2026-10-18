// Issues: the person's own, a search, one with its comments, and the changes
// a person or an agent makes to them.

use std::path::Path;

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::client::{at, Session};
use crate::error::{LinearError, LinearResult};

const ISSUE_FIELDS: &str = "id identifier title url priority priorityLabel updatedAt state { id name type color } assignee { id name displayName } team { id key name } labels { nodes { id name color } }";

const DEFAULT_LIMIT: u64 = 25;
const MAX_LIMIT: u64 = 100;

fn limit(asked: Option<u64>) -> u64 {
    asked.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

/// An issue as GraphQL sends it, with its connections' `nodes` lifted up.
fn flat(mut issue: Value) -> Value {
    lift(&mut issue, "labels");
    lift(&mut issue, "comments");
    if let Some(team) = issue.get_mut("team") {
        lift(team, "states");
    }
    issue
}

fn lift(value: &mut Value, connection: &str) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    if let Some(nodes) = object
        .get(connection)
        .and_then(|connection| connection.get("nodes"))
        .cloned()
    {
        object.insert(connection.to_string(), nodes);
    }
}

fn issues_at(data: &Value, pointer: &str) -> Vec<Value> {
    data.pointer(pointer)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .cloned()
        .map(flat)
        .collect()
}

#[derive(Deserialize)]
pub struct Listing {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub workspace: Option<String>,
}

/// With words, the issues they find; without, the person's open issues.
pub async fn search(data_dir: &Path, input: Listing) -> LinearResult<Vec<Value>> {
    let session = Session::of(data_dir, input.workspace.as_deref()).await?;
    let first = limit(input.limit);
    let words = input.query.as_deref().map(str::trim).unwrap_or_default();
    if words.is_empty() {
        let data = session
            .query(
                &format!("query MyIssues($first: Int!) {{ viewer {{ assignedIssues(first: $first, orderBy: updatedAt, filter: {{ state: {{ type: {{ nin: [\"completed\", \"canceled\"] }} }} }}) {{ nodes {{ {ISSUE_FIELDS} }} }} }} }}"),
                json!({ "first": first }),
            )
            .await?;
        return Ok(issues_at(&data, "/viewer/assignedIssues/nodes"));
    }
    let data = session
        .query(
            &format!("query Search($term: String!, $first: Int!) {{ searchIssues(term: $term, first: $first) {{ nodes {{ {ISSUE_FIELDS} }} }} }}"),
            json!({ "term": words, "first": first }),
        )
        .await?;
    Ok(issues_at(&data, "/searchIssues/nodes"))
}

#[derive(Deserialize)]
pub struct IssueRef {
    /// An identifier such as ENG-123, or Linear's id.
    pub id: String,
    #[serde(default)]
    pub workspace: Option<String>,
}

const ISSUE: &str = "query Issue($id: String!) { issue(id: $id) { id identifier title description url priority priorityLabel createdAt updatedAt branchName state { id name type color } assignee { id name displayName } creator { name } team { id key name states { nodes { id name type color position } } } labels { nodes { id name color } } project { name } cycle { number name } comments(first: 50) { nodes { id body createdAt user { name displayName } } } } }";

fn identifier(input: &IssueRef) -> LinearResult<String> {
    let id = input.id.trim();
    if id.is_empty() {
        return Err(LinearError::BadArg("no issue was named".into()));
    }
    Ok(id.to_string())
}

/// One issue with its description, comments and its team's states.
pub async fn issue(data_dir: &Path, input: IssueRef) -> LinearResult<Value> {
    let session = Session::of(data_dir, input.workspace.as_deref()).await?;
    read(&session, &identifier(&input)?).await
}

async fn read(session: &Session, id: &str) -> LinearResult<Value> {
    let data = session.query(ISSUE, json!({ "id": id })).await?;
    match data.get("issue") {
        Some(issue) if !issue.is_null() => Ok(flat(issue.clone())),
        _ => Err(LinearError::NotFound(format!("linear: no issue {id}"))),
    }
}

#[derive(Deserialize)]
pub struct NewComment {
    pub id: String,
    pub body: String,
    #[serde(default)]
    pub workspace: Option<String>,
}

/// Comments on the issue as the person, in markdown.
pub async fn comment(data_dir: &Path, input: NewComment) -> LinearResult<Value> {
    let body = input.body.trim();
    if body.is_empty() {
        return Err(LinearError::BadArg("the comment is empty".into()));
    }
    let session = Session::of(data_dir, input.workspace.as_deref()).await?;
    let issue = read(&session, input.id.trim()).await?;
    let data = session
        .query(
            "mutation Comment($issueId: String!, $body: String!) { commentCreate(input: { issueId: $issueId, body: $body }) { success comment { id url } } }",
            json!({ "issueId": at(&issue, "/id"), "body": body }),
        )
        .await?;
    Ok(at(&data, "/commentCreate/comment").clone())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub id: String,
    /// A state of the issue's team, by name, such as In Review.
    #[serde(default)]
    pub state: Option<String>,
    /// "me", a name or an email, or empty to unassign.
    #[serde(default)]
    pub assignee: Option<String>,
    /// 0 none, 1 urgent, 2 high, 3 medium, 4 low.
    #[serde(default)]
    pub priority: Option<u8>,
    #[serde(default)]
    pub add_labels: Vec<String>,
    #[serde(default)]
    pub remove_labels: Vec<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub workspace: Option<String>,
}

/// The id of the team state named, matched without regard to case.
pub fn state_id(team: &Value, name: &str) -> LinearResult<String> {
    let states = at(team, "/states").as_array().cloned().unwrap_or_default();
    states
        .iter()
        .find(|state| {
            at(state, "/name")
                .as_str()
                .is_some_and(|known| known.eq_ignore_ascii_case(name.trim()))
        })
        .and_then(|state| at(state, "/id").as_str().map(str::to_string))
        .ok_or_else(|| {
            let names: Vec<&str> = states
                .iter()
                .filter_map(|state| at(state, "/name").as_str())
                .collect();
            LinearError::BadArg(format!(
                "{} has no state {name}; its states are {}",
                at(team, "/key").as_str().unwrap_or("the team"),
                names.join(", ")
            ))
        })
}

async fn assignee_id(session: &Session, who: &str) -> LinearResult<Value> {
    let who = who.trim();
    if who.is_empty() || who.eq_ignore_ascii_case("none") {
        return Ok(Value::Null);
    }
    if who.eq_ignore_ascii_case("me") {
        return Ok(json!(session.workspace.user_id));
    }
    let data = session
        .query(
            "query People($term: String!) { users(first: 20, filter: { or: [{ name: { containsIgnoreCase: $term } }, { displayName: { containsIgnoreCase: $term } }, { email: { containsIgnoreCase: $term } }] }) { nodes { id name displayName email active } } }",
            json!({ "term": who }),
        )
        .await?;
    let people: Vec<Value> = at(&data, "/users/nodes")
        .as_array()
        .into_iter()
        .flatten()
        .filter(|person| at(person, "/active") != false)
        .cloned()
        .collect();
    let exact: Vec<&Value> = people
        .iter()
        .filter(|person| {
            ["name", "displayName", "email"].iter().any(|key| {
                person
                    .get(*key)
                    .and_then(Value::as_str)
                    .is_some_and(|text| text.eq_ignore_ascii_case(who))
            })
        })
        .collect();
    let chosen = match (exact.as_slice(), people.as_slice()) {
        ([one], _) => *one,
        ([], [one]) => one,
        ([], []) => {
            return Err(LinearError::NotFound(format!(
                "linear: nobody called {who}"
            )))
        }
        _ => {
            let names: Vec<&str> = people
                .iter()
                .filter_map(|person| at(person, "/email").as_str())
                .collect();
            return Err(LinearError::BadArg(format!(
                "{who} could be {}; name one by email",
                names.join(", ")
            )));
        }
    };
    Ok(at(chosen, "/id").clone())
}

async fn label_ids(session: &Session, team: &str, names: &[String]) -> LinearResult<Vec<String>> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let data = session
        .query(
            "query Labels($team: ID!) { issueLabels(first: 100, filter: { or: [{ team: { id: { eq: $team } } }, { team: { null: true } }] }) { nodes { id name } } }",
            json!({ "team": team }),
        )
        .await?;
    let known = at(&data, "/issueLabels/nodes")
        .as_array()
        .cloned()
        .unwrap_or_default();
    names
        .iter()
        .map(|name| {
            known
                .iter()
                .find(|label| {
                    at(label, "/name")
                        .as_str()
                        .is_some_and(|known| known.eq_ignore_ascii_case(name.trim()))
                })
                .and_then(|label| at(label, "/id").as_str().map(str::to_string))
                .ok_or_else(|| LinearError::BadArg(format!("no label {name} in this team")))
        })
        .collect()
}

/// Moves, assigns, reprioritises, relabels or renames the issue.
pub async fn update(data_dir: &Path, input: Change) -> LinearResult<Value> {
    let session = Session::of(data_dir, input.workspace.as_deref()).await?;
    let issue = read(&session, input.id.trim()).await?;
    let mut change = Map::new();
    if let Some(state) = &input.state {
        change.insert(
            "stateId".into(),
            json!(state_id(at(&issue, "/team"), state)?),
        );
    }
    if let Some(who) = &input.assignee {
        change.insert("assigneeId".into(), assignee_id(&session, who).await?);
    }
    if let Some(priority) = input.priority {
        if priority > 4 {
            return Err(LinearError::BadArg("priority is 0 to 4".into()));
        }
        change.insert("priority".into(), json!(priority));
    }
    if let Some(title) = input
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
    {
        change.insert("title".into(), json!(title));
    }
    let team = at(&issue, "/team/id").as_str().unwrap_or_default();
    let added = label_ids(&session, team, &input.add_labels).await?;
    if !added.is_empty() {
        change.insert("addedLabelIds".into(), json!(added));
    }
    let removed = label_ids(&session, team, &input.remove_labels).await?;
    if !removed.is_empty() {
        change.insert("removedLabelIds".into(), json!(removed));
    }
    if change.is_empty() {
        return Err(LinearError::BadArg("nothing to change".into()));
    }
    session
        .query(
            "mutation Update($id: String!, $input: IssueUpdateInput!) { issueUpdate(id: $id, input: $input) { success issue { id identifier state { name } assignee { name } } } }",
            json!({ "id": at(&issue, "/id"), "input": change }),
        )
        .await?;
    read(&session, input.id.trim()).await
}

#[derive(Deserialize)]
pub struct NewIssue {
    /// The team's key, such as ENG, or its name.
    pub team: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub priority: Option<u8>,
    #[serde(default)]
    pub workspace: Option<String>,
}

const TEAMS: &str = "query Teams { teams(first: 100) { nodes { id key name states { nodes { id name type position } } } } }";

fn team_named<'a>(teams: &'a [Value], wanted: &str) -> LinearResult<&'a Value> {
    teams
        .iter()
        .find(|team| {
            ["key", "name"].iter().any(|field| {
                team.get(*field)
                    .and_then(Value::as_str)
                    .is_some_and(|text| text.eq_ignore_ascii_case(wanted.trim()))
            })
        })
        .ok_or_else(|| {
            let keys: Vec<&str> = teams
                .iter()
                .filter_map(|team| at(team, "/key").as_str())
                .collect();
            LinearError::BadArg(format!(
                "no team {wanted}; the teams are {}",
                keys.join(", ")
            ))
        })
}

/// The workspace's teams and each one's states.
pub async fn teams(data_dir: &Path, input: crate::auth::WorkspaceRef) -> LinearResult<Vec<Value>> {
    let session = Session::of(data_dir, input.workspace.as_deref()).await?;
    let data = session.query(TEAMS, json!({})).await?;
    Ok(at(&data, "/teams/nodes")
        .as_array()
        .into_iter()
        .flatten()
        .cloned()
        .map(|mut team| {
            lift(&mut team, "states");
            team
        })
        .collect())
}

/// Files a new issue in the team named.
pub async fn create(data_dir: &Path, input: NewIssue) -> LinearResult<Value> {
    let title = input.title.trim();
    if title.is_empty() {
        return Err(LinearError::BadArg("the issue needs a title".into()));
    }
    let teams = teams(
        data_dir,
        crate::auth::WorkspaceRef {
            workspace: input.workspace.clone(),
        },
    )
    .await?;
    let team = team_named(&teams, &input.team)?;
    let session = Session::of(data_dir, input.workspace.as_deref()).await?;
    let mut fields = Map::new();
    fields.insert("teamId".into(), at(team, "/id").clone());
    fields.insert("title".into(), json!(title));
    if let Some(description) = input
        .description
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        fields.insert("description".into(), json!(description));
    }
    if let Some(who) = &input.assignee {
        fields.insert("assigneeId".into(), assignee_id(&session, who).await?);
    }
    if let Some(state) = &input.state {
        fields.insert("stateId".into(), json!(state_id(team, state)?));
    }
    if let Some(priority) = input.priority {
        fields.insert("priority".into(), json!(priority.min(4)));
    }
    let data = session
        .query(
            "mutation Create($input: IssueCreateInput!) { issueCreate(input: $input) { success issue { id identifier title url } } }",
            json!({ "input": fields }),
        )
        .await?;
    Ok(at(&data, "/issueCreate/issue").clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connections_are_lifted_out_of_their_nodes() {
        let issue = flat(
            json!({ "identifier": "ENG-1", "labels": { "nodes": [{ "name": "bug" }] }, "comments": { "nodes": [] }, "team": { "key": "ENG", "states": { "nodes": [{ "name": "Todo" }] } } }),
        );
        assert_eq!(issue["labels"], json!([{ "name": "bug" }]));
        assert_eq!(issue["comments"], json!([]));
        assert_eq!(issue["team"]["states"], json!([{ "name": "Todo" }]));
    }

    #[test]
    fn a_state_is_found_by_name_and_a_wrong_one_lists_the_teams_states() {
        let team = json!({ "key": "ENG", "states": [{ "id": "s1", "name": "Todo" }, { "id": "s2", "name": "In Review" }] });
        assert_eq!(state_id(&team, "in review").ok().as_deref(), Some("s2"));
        let error = state_id(&team, "Shipped").expect_err("unknown");
        assert!(error.to_string().contains("Todo, In Review"), "{error}");
    }

    #[test]
    fn a_team_is_found_by_key_or_name() {
        let teams = [
            json!({ "id": "t1", "key": "ENG", "name": "Engineering" }),
            json!({ "id": "t2", "key": "DES", "name": "Design" }),
        ];
        assert_eq!(
            team_named(&teams, "eng")
                .ok()
                .map(|team| team["id"].clone()),
            Some(json!("t1"))
        );
        assert_eq!(
            team_named(&teams, "Design")
                .ok()
                .map(|team| team["id"].clone()),
            Some(json!("t2"))
        );
        assert!(team_named(&teams, "OPS")
            .expect_err("unknown")
            .to_string()
            .contains("ENG, DES"));
    }

    #[test]
    fn a_limit_stays_between_one_and_a_hundred() {
        assert_eq!(
            (limit(None), limit(Some(0)), limit(Some(500))),
            (25, 1, 100)
        );
    }
}
