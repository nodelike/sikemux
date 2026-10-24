//! What the Jira pane shows: a list of issues with its statuses counted, the
//! person's spaces, a board narrowed to the people picked, and one issue with
//! its comments and edits together. The core keeps each and reads it again on
//! the pace the manifest gives.

use serde::Deserialize;
use sikemux_plugin_api::jira::{
    Activity, Board, BoardPage, BoardPerson, BoardView, Change, Column, Comment, IssueList,
    IssueListKind, IssueSummary, IssueView, Space, Spaces, StatusCount, Swimlane,
};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use time::macros::format_description;
use time::OffsetDateTime;

use crate::boards::{self, BoardRequest, BoardsRequest};
use crate::error::JiraResult;
use crate::issues::{self, IssueRequest, Search, SiteRequest};

/// How many issues a list shows.
const LIST_LIMIT: u64 = 50;

const MINE_JQL: &str = "assignee = currentUser() AND (statusCategory != Done OR updated >= -14d) ORDER BY updated DESC";
const REPORTED_JQL: &str = "reporter = currentUser() AND (statusCategory != Done OR updated >= -14d) ORDER BY updated DESC";
const WATCHING_JQL: &str =
    "watcher = currentUser() AND statusCategory != Done ORDER BY updated DESC";
const RECENT_JQL: &str = "issuekey in issueHistory() ORDER BY lastViewed DESC";
const SPRINT_JQL: &str = "sprint in openSprints() ORDER BY Rank ASC";

/// Typed text reads as JQL once it compares, orders or tests for nothing.
fn reads_as_jql(text: &str) -> bool {
    if text.contains(['=', '~', '<', '>']) {
        return true;
    }
    let spaced = text.to_lowercase().replace('(', " ( ");
    let words: Vec<&str> = spaced
        .split_whitespace()
        .map(|word| {
            if word == "(" {
                word
            } else {
                word.trim_matches(|char: char| !char.is_alphanumeric())
            }
        })
        .collect();
    words.windows(2).any(|pair| {
        matches!(
            pair,
            ["order", "by"] | ["in", "("] | ["is", "empty" | "null"]
        )
    }) || words
        .windows(3)
        .any(|three| matches!(three, ["is", "not", "empty" | "null"]))
}

/// A project key as JQL can hold it, in quotes in case the key is also a JQL word.
fn project_clause(key: &str) -> String {
    let key: String = key
        .chars()
        .filter(|char| char.is_ascii_alphanumeric() || *char == '_')
        .collect();
    format!("project = \"{key}\"")
}

/// What is typed in the search box: JQL when it reads as JQL, otherwise words
/// to find in any issue's text.
pub fn search_jql(typed: &str) -> String {
    let text = typed.trim();
    if reads_as_jql(text) {
        return text.to_owned();
    }
    let quoted = text.replace('\\', "\\\\").replace('"', "\\\"");
    format!("text ~ \"{quoted}\" ORDER BY updated DESC")
}

pub fn jql_of(list: &IssueListKind) -> String {
    match list {
        IssueListKind::Mine => MINE_JQL.to_owned(),
        IssueListKind::Reported => REPORTED_JQL.to_owned(),
        IssueListKind::Watching => WATCHING_JQL.to_owned(),
        IssueListKind::Recent => RECENT_JQL.to_owned(),
        IssueListKind::Sprint => SPRINT_JQL.to_owned(),
        IssueListKind::Project { key } => format!(
            "{} AND statusCategory != Done ORDER BY updated DESC",
            project_clause(key)
        ),
        IssueListKind::Unassigned { key } => format!(
            "{} AND assignee is EMPTY AND statusCategory != Done ORDER BY created DESC",
            project_clause(key)
        ),
        IssueListKind::Filter { jql } => jql.clone(),
        IssueListKind::Search { text } => search_jql(text),
    }
}

/// Work in progress first, then work not started, and finished work last.
fn category_rank(category: &str) -> usize {
    match category {
        "indeterminate" => 0,
        "new" => 1,
        "done" => 3,
        _ => 2,
    }
}

pub fn status_counts(issues: &[IssueSummary]) -> Vec<StatusCount> {
    let mut counts: Vec<StatusCount> = Vec::new();
    for issue in issues {
        match counts.iter_mut().find(|each| each.status == issue.status) {
            Some(seen) => seen.count += 1,
            None => counts.push(StatusCount {
                status: issue.status.clone(),
                category: issue.status_category.clone(),
                count: 1,
            }),
        }
    }
    counts.sort_by(|a, b| {
        category_rank(&a.category)
            .cmp(&category_rank(&b.category))
            .then_with(|| a.status.cmp(&b.status))
    });
    counts
}

/// Everyone with a card on the board, busiest first, and how many cards nobody has.
pub fn board_people(board: &BoardView) -> (Vec<BoardPerson>, u32) {
    let mut people: Vec<BoardPerson> = Vec::new();
    let mut unassigned = 0;
    for issue in board.columns.iter().flat_map(|column| &column.issues) {
        let Some(assignee) = &issue.assignee else {
            unassigned += 1;
            continue;
        };
        match people
            .iter_mut()
            .find(|each| each.account_id == assignee.account_id)
        {
            Some(seen) => seen.count += 1,
            None => people.push(BoardPerson {
                account_id: assignee.account_id.clone(),
                name: assignee.name.clone(),
                avatar_url: assignee.avatar_url.clone(),
                count: 1,
            }),
        }
    }
    people.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    (people, unassigned)
}

pub fn swimlanes(columns: &[Column]) -> Vec<Swimlane> {
    let mut lanes: Vec<Swimlane> = Vec::new();
    for (index, column) in columns.iter().enumerate() {
        for issue in &column.issues {
            let id = issue
                .assignee
                .as_ref()
                .map(|person| person.account_id.clone())
                .unwrap_or_default();
            if !lanes.iter().any(|lane| lane.id == id) {
                lanes.push(Swimlane {
                    id: id.clone(),
                    person: issue.assignee.clone(),
                    count: 0,
                    columns: columns
                        .iter()
                        .map(|each| Column {
                            issues: Vec::new(),
                            ..each.clone()
                        })
                        .collect(),
                });
            }
            let Some(lane) = lanes.iter_mut().find(|lane| lane.id == id) else {
                continue;
            };
            lane.count += 1;
            if let Some(column) = lane.columns.get_mut(index) {
                column.issues.push(issue.clone());
            }
        }
    }
    lanes.sort_by(|a, b| {
        a.person
            .is_none()
            .cmp(&b.person.is_none())
            .then_with(|| b.count.cmp(&a.count))
            .then_with(|| {
                let name = |lane: &Swimlane| lane.person.as_ref().map(|person| person.name.clone());
                name(a).cmp(&name(b))
            })
    });
    lanes
}

/// Jira writes times as `2026-10-21T09:30:00.000+0000`, each with the
/// offset of whoever's site it is.
fn instant(stamp: &str) -> Option<OffsetDateTime> {
    let format = format_description!(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond][offset_hour sign:mandatory][offset_minute]"
    );
    OffsetDateTime::parse(stamp, format).ok()
}

/// An issue's comments and edits together, newest first.
pub fn activity(comments: Vec<Comment>, history: Vec<Change>) -> Vec<Activity> {
    let mut all: Vec<(Option<OffsetDateTime>, Activity)> = comments
        .into_iter()
        .map(|comment| (instant(&comment.created), Activity::Comment { comment }))
        .chain(
            history
                .into_iter()
                .map(|change| (instant(&change.created), Activity::Change { change })),
        )
        .collect();
    all.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    all.into_iter().map(|(_, entry)| entry).collect()
}

#[derive(Deserialize)]
pub struct ListRequest {
    pub list: IssueListKind,
    #[serde(default)]
    pub site: Option<String>,
}

pub async fn list_view(data_dir: &Path, request: ListRequest) -> JiraResult<IssueList> {
    let found = issues::search(
        data_dir,
        Search {
            jql: jql_of(&request.list),
            limit: Some(LIST_LIMIT),
            site: request.site,
            next: None,
            comments: false,
        },
    )
    .await?;
    let issues: Vec<IssueSummary> = found.issues.into_iter().map(|hit| hit.summary).collect();
    Ok(IssueList {
        statuses: status_counts(&issues),
        issues,
    })
}

pub async fn spaces_view(data_dir: &Path, request: SiteRequest) -> JiraResult<Spaces> {
    let site = request.site;
    let (projects, boards, filters) = tokio::join!(
        issues::projects(data_dir, SiteRequest { site: site.clone() }),
        boards::boards(
            data_dir,
            BoardsRequest {
                site: site.clone(),
                ..Default::default()
            }
        ),
        issues::filters(data_dir, SiteRequest { site }),
    );
    let projects = projects?;
    let (boards, boards_failed) = match boards {
        Ok(boards) => (boards, None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    };
    let home = |board: &Board| {
        board
            .project_key
            .clone()
            .or_else(|| board.project.clone())
            .unwrap_or_default()
    };
    let keys: HashSet<&str> = projects
        .iter()
        .map(|project| project.key.as_str())
        .collect();
    let other_boards = boards
        .iter()
        .filter(|board| !keys.contains(home(board).as_str()))
        .cloned()
        .collect();
    let mut by_home: HashMap<String, Vec<Board>> = HashMap::new();
    for board in boards {
        by_home.entry(home(&board)).or_default().push(board);
    }
    Ok(Spaces {
        spaces: projects
            .into_iter()
            .map(|project| Space {
                boards: by_home.remove(&project.key).unwrap_or_default(),
                project,
            })
            .collect(),
        other_boards,
        filters: filters.unwrap_or_default(),
        boards_failed,
    })
}

pub async fn board_view(data_dir: &Path, request: BoardRequest) -> JiraResult<BoardPage> {
    let narrowed = request.mine || request.unassigned || !request.people.is_empty();
    let whole = boards::board(
        data_dir,
        BoardRequest {
            id: request.id,
            site: request.site.clone(),
            ..Default::default()
        },
    )
    .await?;
    let board = if narrowed {
        boards::board(data_dir, request).await?
    } else {
        whole.clone()
    };
    let (people, unassigned) = board_people(&whole);
    Ok(BoardPage {
        lanes: swimlanes(&board.columns),
        board,
        people,
        unassigned,
    })
}

pub async fn issue_view(data_dir: &Path, request: IssueRequest) -> JiraResult<IssueView> {
    let detail = issues::issue(data_dir, request).await?;
    let mut comments = detail.comments;
    comments.reverse();
    Ok(IssueView {
        issue: detail.summary,
        project: detail.project,
        reporter: detail.reporter,
        labels: detail.labels,
        created: detail.created,
        description: detail.description,
        transitions: detail.transitions,
        subtasks: detail.subtasks,
        blocked_by: detail.blocked_by,
        blocks: detail.blocks,
        comment_count: detail.comment_count,
        activity: activity(comments, detail.history),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sikemux_plugin_api::jira::Person;

    fn issue(key: &str, status: &str, category: &str, assignee: Option<&str>) -> IssueSummary {
        IssueSummary {
            key: key.to_owned(),
            summary: key.to_owned(),
            status: status.to_owned(),
            status_category: category.to_owned(),
            priority: None,
            assignee: assignee.map(|name| Person {
                account_id: name.to_lowercase(),
                name: name.to_owned(),
                avatar_url: None,
            }),
            issue_type: None,
            sprint: None,
            updated: None,
            due: None,
            parent: None,
            url: String::new(),
        }
    }

    #[test]
    fn lists_a_projects_open_and_unassigned_issues_keeping_only_what_a_key_can_hold() {
        let project = IssueListKind::Project {
            key: "CIQ".to_owned(),
        };
        assert_eq!(
            jql_of(&project),
            "project = \"CIQ\" AND statusCategory != Done ORDER BY updated DESC"
        );
        let sneaky = IssueListKind::Unassigned {
            key: "CIQ\" OR 1=1".to_owned(),
        };
        assert_eq!(
            jql_of(&sneaky),
            "project = \"CIQOR11\" AND assignee is EMPTY AND statusCategory != Done ORDER BY created DESC"
        );
    }

    #[test]
    fn runs_jql_as_it_is_and_finds_plain_words_in_issue_text() {
        assert_eq!(
            search_jql("project = ABC AND status = 'To Do'"),
            "project = ABC AND status = 'To Do'"
        );
        assert_eq!(
            search_jql("assignee in (currentUser()) order by created"),
            "assignee in (currentUser()) order by created"
        );
        assert_eq!(
            search_jql(" ankit "),
            "text ~ \"ankit\" ORDER BY updated DESC"
        );
        assert_eq!(
            search_jql("login \"race\""),
            "text ~ \"login \\\"race\\\"\" ORDER BY updated DESC"
        );
        let typed = IssueListKind::Search {
            text: "login bug".to_owned(),
        };
        assert_eq!(jql_of(&typed), "text ~ \"login bug\" ORDER BY updated DESC");
        assert_eq!(
            search_jql("bug in login is slow"),
            "text ~ \"bug in login is slow\" ORDER BY updated DESC"
        );
        assert_eq!(search_jql("assignee is empty"), "assignee is empty");
    }

    #[test]
    fn counts_each_status_work_in_progress_first_and_finished_work_last() {
        let issues = [
            issue("CIQ-1", "Deployed", "done", None),
            issue("CIQ-2", "To Do", "new", None),
            issue("CIQ-3", "In Progress", "indeterminate", None),
            issue("CIQ-4", "Deployed", "done", None),
            issue("CIQ-5", "Done", "done", None),
        ];
        let counts = status_counts(&issues);
        let counted: Vec<(&str, u32)> = counts
            .iter()
            .map(|each| (each.status.as_str(), each.count))
            .collect();
        assert_eq!(
            counted,
            [
                ("In Progress", 1),
                ("To Do", 1),
                ("Deployed", 2),
                ("Done", 1)
            ]
        );
    }

    #[test]
    fn lists_everyone_with_a_card_busiest_first_and_counts_the_cards_nobody_has() {
        let column = |name: &str, issues: Vec<IssueSummary>| Column {
            name: name.to_owned(),
            status_ids: Vec::new(),
            issues,
        };
        let board = BoardView {
            id: 1,
            name: "B".to_owned(),
            kind: "kanban".to_owned(),
            sprint: None,
            truncated: false,
            columns: vec![
                column(
                    "To Do",
                    vec![
                        issue("CIQ-1", "", "", Some("Vivek")),
                        issue("CIQ-2", "", "", None),
                    ],
                ),
                column(
                    "Done",
                    vec![
                        issue("CIQ-3", "", "", Some("Kiran")),
                        issue("CIQ-4", "", "", Some("Vivek")),
                    ],
                ),
            ],
        };
        let (people, unassigned) = board_people(&board);
        let named: Vec<(&str, u32)> = people
            .iter()
            .map(|person| (person.account_id.as_str(), person.count))
            .collect();
        assert_eq!(named, [("vivek", 2), ("kiran", 1)]);
        assert_eq!(unassigned, 1);
    }
}
