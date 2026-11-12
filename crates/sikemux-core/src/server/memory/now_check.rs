//! Rules of the person's that leave the list without being lost. When the person repeats a
//! correction a rule of theirs already makes, the project's chief names the rule as said again,
//! and where a check (lint, compiler or CI) can express it, has a worker land one and retires the
//! rule as now a check, naming the check's commit. An agent may also retire or reword a rule of
//! the person's in a turn the person's own words started, never the rule an Overrule kept. Each
//! leaves a line with Undo in the chief's chat, or the acting agent's where there is no chief, and
//! a rule that leaves is kept apart, with who and when, until the person brings it back.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::chat::RuleChange;
use sikemux_view::memory::{Learned, MemoryRule};

use super::{tell_clients, Core, State};
use crate::git::Git;
use crate::server::parts::Parts;

/// A rule of the person's that left the list, kept apart.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetAside {
    pub rule: MemoryRule,
    /// A check in the repository holds it now: the landed commit that made the check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    pub at: u64,
    /// The agent that set it aside, by id.
    pub by: String,
}

const NOW_A_CHECK: &str = "now a check";
const SAID_AGAIN: &str = "said again";

/// What `wrong` on a rule of the person's asks of the core, when it is one of the chief's two
/// labels: the rule said again, or now a check with the commit named after the label.
pub(super) fn labelled(
    core: &Core,
    project: &str,
    id: u64,
    said: &str,
    by: &Learned,
) -> Option<Result<(), String>> {
    let said = said.trim();
    let label = |label: &str| {
        (said.get(..label.len()))
            .filter(|start| start.eq_ignore_ascii_case(label))
            .map(|_| &said[label.len()..])
    };
    if let Some(rest) = label(NOW_A_CHECK) {
        return Some(retire(core, project, id, rest, by));
    }
    label(SAID_AGAIN).map(|_| said_again(core, project, id, by))
}

fn chief_of(core: &Core, project: &str, by: &Learned, doing: &str) -> Result<String, String> {
    (by.agent.clone())
        .filter(|agent| crate::server::chief::chief_at(core, project).as_ref() == Some(agent))
        .ok_or_else(|| format!("Only this project's chief {doing}; leave the rule as it is"))
}

/// The chief heard the person repeat a correction their rule `id` already makes. Counted once a
/// rule in each of the chief's turns; the rule is unchanged.
fn said_again(core: &Core, project: &str, id: u64, by: &Learned) -> Result<(), String> {
    let chief = chief_of(core, project, by, "names a rule said again")?;
    let first = {
        let mut state = core.memory().state.locked();
        let persons = (state.applying(project))
            .flat_map(|held| held.rules.iter())
            .find(|rule| rule.id == id)
            .is_some_and(|rule| rule.learned.agent.is_none());
        if !persons {
            return Err(format!(
                "#{id} is an agent's note, not a rule of the person's, so it is not counted"
            ));
        }
        let (turn, rules) = state.said_again.entry(chief).or_default();
        if *turn != by.turn {
            *turn = by.turn;
            rules.clear();
        }
        rules.insert(id)
    };
    if first {
        core.activity_log().core_used("rule.saidAgain");
    }
    Ok(())
}

/// The chief `by` retires rule `id` of `project` as now a check, `rest` naming the landed commit
/// that made it. Only the project's chief may, only for a rule kept from the person's own words
/// for the project itself, never one an Overrule kept, and only with a commit newer than the rule
/// that is in the project's repository and has landed there.
fn retire(core: &Core, project: &str, id: u64, rest: &str, by: &Learned) -> Result<(), String> {
    let chief = chief_of(core, project, by, "retires a rule as now a check")?;
    let rule = {
        let state = core.memory().state.locked();
        if state.promoting == Some(id) {
            return Err(format!("#{id} is being written into AGENTS.md"));
        }
        (state.held.get(project))
            .and_then(|held| held.rules.iter().find(|rule| rule.id == id).cloned())
            .ok_or_else(|| {
                format!(
                    "#{id} is not a rule kept for this project itself, so it cannot become a check"
                )
            })?
    };
    if rule.learned.agent.is_some() || rule.sensitive {
        return Err(format!(
            "#{id} was not kept from the person's own words, or is sensitive, so it stays a rule"
        ));
    }
    if crate::server::chief::kept_by_overrule(core, project, id) {
        return Err(format!(
            "#{id} is the rule an Overrule kept, about who decides, so it stays a rule"
        ));
    }
    let named = (rest.trim_start_matches([':', ' ']).split_whitespace().next())
        .filter(|word| (7..=40).contains(&word.len()) && word.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or("Name the landed commit that made the check right after the label, as wrong \"now a check: 3f2a9c1\"")?;
    let git = || Git::quick(Path::new(project));
    let one_repository = (git().output(&["rev-parse", "--show-toplevel"]).ok())
        .is_some_and(|output| output.status.success());
    if !one_repository {
        return Err("This project is a folder of repositories, not one, so no commit here can stand for a check; the rule stays".into());
    }
    let shown = (git()
        .output(&[
            "show",
            "-s",
            "--format=%H %ct %P",
            &format!("{named}^{{commit}}"),
        ])
        .ok())
    .filter(|output| output.status.success())
    .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    .ok_or_else(|| format!("{named} is no commit in this project's repository"))?;
    let mut fields = shown.split_whitespace();
    let commit = fields.next().unwrap_or_default().to_owned();
    let made_ms = fields
        .next()
        .and_then(|at| at.parse::<u64>().ok())
        .unwrap_or(0)
        * 1000;
    if fields.next().is_none() {
        return Err(format!(
            "{named} is the repository's first commit, which no check for this rule can be"
        ));
    }
    if made_ms <= rule.learned.at {
        return Err(format!(
            "{named} is older than #{id}, so it cannot be the check for it; land one made now"
        ));
    }
    if !crate::server::turn_state::landed(core, project, None, &commit) {
        return Err(format!(
            "{named} has not landed on this project's branch; land the check first"
        ));
    }
    {
        let mut state = core.memory().state.locked();
        let held = (state.held.get_mut(project)).ok_or("the folder went while it was changed")?;
        let at = (held.rules.iter())
            .position(|kept| kept.id == id)
            .ok_or_else(|| format!("#{id} is no longer kept"))?;
        let rule = held.rules.remove(at);
        held.set_aside.push(SetAside {
            rule,
            commit: Some(commit.clone()),
            at: epoch_ms(),
            by: chief.clone(),
        });
        core.memory().save(&mut state, project);
    }
    tell_clients(core, false);
    core.activity_log().core_used("rule.nowCheck");
    let line = line(core, project, &rule, &chief, Some(commit), None);
    crate::server::chief::rule_changed(core, line, &chief);
    Ok(())
}

/// The chief's line for rule `rule` of `project` that `agent` changed.
pub(super) fn line(
    core: &Core,
    project: &str,
    rule: &MemoryRule,
    agent: &str,
    commit: Option<String>,
    was: Option<String>,
) -> RuleChange {
    RuleChange {
        project: project.to_owned(),
        rule: rule.id,
        text: crate::text::one_line(&rule.text),
        commit,
        was: was.map(|was| crate::text::one_line(&was)),
        by: core.names().name_or_id(agent),
        restored: false,
        back_by: None,
    }
}

/// Whether an agent may retire or reword rule `rule` of the person's now: only in a turn the
/// person's own words started, never a sensitive one, and never the one an Overrule kept.
pub(super) fn agent_may_change(
    rule: &MemoryRule,
    overruled: bool,
    persons_turn: bool,
) -> Result<(), String> {
    let id = rule.id;
    if overruled {
        return Err(format!(
            "#{id} is the rule an Overrule kept, about who decides, and no agent changes it; the person takes it back with Undo on its Overrule line or in the Playbook"
        ));
    }
    if rule.sensitive {
        return Err(format!(
            "#{id} is the person's sensitive rule; only they change it, on the Playbook"
        ));
    }
    match persons_turn {
        true => Ok(()),
        false => Err(format!(
            "#{id} is the person's own rule; retire or reword it only in a turn the person's own words started, as they asked"
        )),
    }
}

/// The rules of the person's set aside for `project`, oldest first.
pub(crate) fn set_aside(core: &Core, project: &str) -> Vec<SetAside> {
    let state = core.memory().state.locked();
    (state.applying(project))
        .flat_map(|held| held.set_aside.iter().cloned())
        .collect()
}

/// Why a rule was set aside, in words for agents.
pub(crate) fn why(apart: &SetAside) -> String {
    match &apart.commit {
        Some(commit) => format!("now a check: {}", commit.get(..7).unwrap_or(commit)),
        None => "in a turn the person started".to_owned(),
    }
}

pub(super) fn is_set_aside(core: &Core, project: &str, id: u64) -> bool {
    let state = core.memory().state.locked();
    let found = (state.applying(project))
        .any(|held| held.set_aside.iter().any(|apart| apart.rule.id == id));
    found
}

/// Whether the person's own words started `agent`'s turn now.
pub(super) fn persons_turn(core: &Core, agent: &str) -> bool {
    (core.chats().get(agent)).is_some_and(|chat| !chat.feed.person_said().is_empty())
}

/// An agent brings back rule `id` of the person's in a turn the person's own words started.
pub(super) fn bring_back_asked(
    core: &Core,
    project: &str,
    id: u64,
    by: &Learned,
) -> Result<(), String> {
    let agent = (by.agent.as_deref()).ok_or("Only an agent brings a rule back this way")?;
    if !persons_turn(core, agent) {
        return Err(
            "Bring a rule back only when the person asks for it; their own words did not start this turn"
                .into(),
        );
    }
    bring_back(core, project, id, Some(agent)).map(|_| ())
}

/// The person keeps `text` as a rule: when it is the words of a rule of theirs set aside, that
/// rule comes back whole, its id and when they said it kept, marked sensitive if they say so.
/// None when no such rule is set aside.
pub(super) fn bring_back_words(
    core: &Core,
    project: &str,
    text: &str,
    sensitive: bool,
) -> Option<Result<(), String>> {
    let same = crate::text::one_line(text);
    let id = (set_aside(core, project).into_iter())
        .find(|apart| crate::text::one_line(&apart.rule.text) == same)?
        .rule
        .id;
    Some(bring_back(core, project, id, None).map(|folder| {
        if sensitive {
            let mut state = core.memory().state.locked();
            let rule = (state.held.get_mut(&folder))
                .and_then(|held| held.rules.iter_mut().find(|rule| rule.id == id));
            if let Some(rule) = rule {
                rule.sensitive = true;
                core.memory().save(&mut state, &folder);
            }
        }
    }))
}

/// The person keeps `new` as a rule when the same words are kept already, as an Undo on a line
/// whose rule came back another way does: the line in the chief's chat says it is back.
pub(super) fn kept_again(core: &Core, project: &str, new: &super::NewMemory) -> bool {
    let super::NewMemory::Rule {
        text,
        goal: None,
        folder: None,
        ..
    } = new
    else {
        return false;
    };
    let same = crate::text::one_line(text);
    let kept = {
        let state = core.memory().state.locked();
        let found = (state.applying(project))
            .flat_map(|held| held.rules.iter())
            .find(|rule| rule.learned.agent.is_none() && crate::text::one_line(&rule.text) == same)
            .cloned();
        found
    };
    let Some(rule) = kept else {
        return false;
    };
    if let Some(chief) = crate::server::chief::chief_at(core, project) {
        let mut back = line(core, project, &rule, &chief, None, None);
        back.restored = true;
        crate::server::chief::rule_back(core, back, &chief);
    }
    true
}

/// Brings rule `id` of the person's back to the list, unless the same words are kept again
/// already, and the line for it says it is back: brought back by `agent`, or by the person.
/// Answers the folder it is kept for.
fn bring_back(core: &Core, project: &str, id: u64, agent: Option<&str>) -> Result<String, String> {
    let (folder, apart) = {
        let mut state = core.memory().state.locked();
        let (folder, apart) = take(&mut state, project, id)?;
        let words = apart.rule.text.to_lowercase();
        let kept_again = (state.applying(project)).any(|held| {
            held.rules
                .iter()
                .any(|rule| rule.text.to_lowercase() == words)
        });
        let held = (state.held.get_mut(&folder)).ok_or("the folder went while it was changed")?;
        if !kept_again {
            held.rules.push(apart.rule.clone());
        }
        core.memory().save(&mut state, &folder);
        (folder, apart)
    };
    tell_clients(core, false);
    let mut back = line(
        core,
        project,
        &apart.rule,
        &apart.by,
        apart.commit.clone(),
        None,
    );
    back.restored = true;
    back.back_by = agent.map(|agent| core.names().name_or_id(agent));
    crate::server::chief::rule_back(core, back.clone(), &apart.by);
    let chief = crate::server::chief::chief_at(core, project);
    if let Some(agent) = agent.filter(|agent| chief.is_none() && *agent != apart.by) {
        crate::server::chief::rule_back(core, back, agent);
    }
    Ok(folder)
}

/// Takes rule `id` out of what is set aside for `project`; answers the folder it is kept for, and
/// what was set aside.
fn take(state: &mut State, project: &str, id: u64) -> Result<(String, SetAside), String> {
    let folder = (state.applying(project))
        .find(|held| held.set_aside.iter().any(|apart| apart.rule.id == id))
        .map(|held| held.folder.clone())
        .ok_or_else(|| format!("#{id} is not a rule set aside for this project"))?;
    let held = (state.held.get_mut(&folder)).ok_or("the folder went while it was changed")?;
    let at = (held.set_aside.iter())
        .position(|apart| apart.rule.id == id)
        .ok_or("the rule went while it was changed")?;
    Ok((folder, held.set_aside.remove(at)))
}
