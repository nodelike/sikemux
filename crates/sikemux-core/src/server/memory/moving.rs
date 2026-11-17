//! The person moves a rule or knowledge between their own store and the project's, keeping its
//! id, and the chief's chat says so with Undo, which moves it back. The chief offers the move on
//! a rule of the person's that names something of this repository, and on a project's rule the
//! person keeps for another project too.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;
use sikemux_view::chat::RuleChange;
use sikemux_view::memory::{Knowledge, Learned, MemoryRule, MoveOffer, Store};

use super::{holding, tell_clients, Core, Held, State, MAX_ENTRIES};
use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;

/// A rule of another project's in the same words as one moved into the person's store, kept to
/// put back when that move is undone.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Absorbed {
    /// The rule that moved into the person's store.
    into: u64,
    /// The store it was taken from.
    key: String,
    rule: MemoryRule,
}

/// A rule or knowledge on its way to the other store, as it goes between hosts.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum Moving {
    Rule(Box<MemoryRule>),
    Knowledge(Box<Knowledge>),
}

impl Moving {
    fn text(&self) -> &str {
        match self {
            Moving::Rule(rule) => &rule.text,
            Moving::Knowledge(known) => &known.text,
        }
    }

    pub(crate) fn id(&self) -> u64 {
        match self {
            Moving::Rule(rule) => rule.id,
            Moving::Knowledge(known) => known.id,
        }
    }

    fn moved(&mut self) -> &mut Option<Learned> {
        match self {
            Moving::Rule(rule) => &mut rule.moved,
            Moving::Knowledge(known) => &mut known.moved,
        }
    }

    fn kept_in(&self, held: &Held) -> bool {
        let text = self.text();
        match self {
            Moving::Rule(_) => (held.rules.iter()).any(|rule| rule.text.eq_ignore_ascii_case(text)),
            Moving::Knowledge(_) => {
                (held.knowledge.iter()).any(|known| known.text.eq_ignore_ascii_case(text))
            }
        }
    }
}

/// The signed-in person's first name, as their account gives it.
pub(crate) fn first_name(core: &Core) -> Option<String> {
    let name = core.account().shown()?.name?;
    name.split_whitespace().next().map(str::to_owned)
}

/// `learned` naming the signed-in person, when the words were theirs.
pub(super) fn named(core: &Core, mut learned: Learned) -> Learned {
    if learned.agent.is_none() && learned.person.is_none() {
        learned.person = first_name(core);
    }
    learned
}

/// Who rewords an entry, keeping who first added it and when.
pub(super) fn reworded(first: &Learned, by: Learned) -> Learned {
    Learned {
        at: first.at,
        person: first.person.clone().or(by.person),
        ..by
    }
}

fn words(text: &str) -> String {
    crate::text::one_line(text).to_lowercase()
}

/// A project as the person knows it, from its store's key: the last part of its identity or
/// folder.
fn project_name(key: &str) -> String {
    let last = key
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .unwrap_or(key);
    last.trim_end_matches(".git").to_owned()
}

/// What decides the chief's offers on one project's Playbook.
pub(super) struct Offers<'a> {
    folder: &'a Path,
    /// The person's rules other projects keep, by their words, with the first such project.
    elsewhere: HashMap<String, String>,
}

impl<'a> Offers<'a> {
    pub(super) fn new(state: &State, project: &'a str) -> Self {
        let own = state.key_of(project);
        let elsewhere = (state.held.values())
            .filter(|held| held.store.is_project() && held.key != own)
            .flat_map(|held| {
                (held.rules.iter())
                    .filter(|rule| rule.learned.agent.is_none())
                    .map(|rule| (words(&rule.text), project_name(&held.key)))
            })
            .collect();
        Self {
            folder: Path::new(project),
            elsewhere,
        }
    }

    pub(super) fn for_rule(&self, held: &Held, rule: &MemoryRule) -> Option<MoveOffer> {
        if rule.learned.agent.is_some() || held.not_now.contains(&rule.id) {
            return None;
        }
        match held.store {
            Store::Personal => names_here(&rule.text, self.folder).then_some(MoveOffer::AboutHere),
            Store::Project => {
                (self.elsewhere.get(&words(&rule.text))).map(|elsewhere| MoveOffer::SaidElsewhere {
                    elsewhere: elsewhere.clone(),
                })
            }
        }
    }
}

/// Whether `text` names the project in `folder`, or a file or folder inside it.
fn names_here(text: &str, folder: &Path) -> bool {
    let name = (folder.file_name().and_then(|name| name.to_str())).map(str::to_lowercase);
    let path_like = |word: &str| {
        (word.contains('/') || word.contains('.'))
            && !word.starts_with('/')
            && !word.starts_with('~')
            && !word.contains("..")
            && !word.contains("://")
    };
    (text.split(|c: char| c.is_whitespace() || "`'\"()[]{},;:!?*".contains(c)))
        .map(|word| word.trim_end_matches('.'))
        .filter(|word| word.chars().count() >= 3)
        .any(|word| {
            name.as_deref() == Some(word.to_lowercase().as_str())
                || (path_like(word) && folder.join(word.trim_end_matches('/')).exists())
        })
}

fn take(held: &mut Held, id: u64) -> Option<Moving> {
    if let Some(at) = held.rules.iter().position(|rule| rule.id == id) {
        return Some(Moving::Rule(Box::new(held.rules.remove(at))));
    }
    let at = held.knowledge.iter().position(|known| known.id == id)?;
    Some(Moving::Knowledge(Box::new(held.knowledge.remove(at))))
}

fn put(held: &mut Held, entry: Moving) {
    match entry {
        Moving::Rule(rule) => held.rules.push(*rule),
        Moving::Knowledge(known) => held.knowledge.push(*known),
    }
}

/// Moves entry `id` into `to`. Moving a project's rule into the person's store takes in other
/// projects' rules in the same words; moving it back puts them back.
pub(super) fn move_to(
    core: &Core,
    project: &str,
    id: u64,
    to: Store,
    by: Learned,
) -> Result<(), String> {
    let by = named(core, by);
    let mut state = core.memory().state.locked();
    if state.promoting == Some(id) {
        return Err(format!("#{id} is being written into AGENTS.md"));
    }
    let from_key = state.key_holding(project, id)?;
    let to_key = state.key_for(project, to);
    if from_key == to_key {
        return Err(format!("#{id} is kept there already"));
    }
    let target = holding(&mut state, &to_key, to)?;
    if target.len() >= MAX_ENTRIES {
        return Err(format!(
            "That store already keeps {MAX_ENTRIES} rules and knowledge; retire some first"
        ));
    }
    let from = (state.held.get_mut(&from_key)).ok_or("the store went while it was changed")?;
    let mut entry = take(from, id).ok_or("the entry went while it was moved")?;
    from.not_now.remove(&id);
    if let Some(target) = state
        .held
        .get(&to_key)
        .filter(|target| entry.kept_in(target))
    {
        let kept = target.key.clone();
        if let Some(from) = state.held.get_mut(&from_key) {
            put(from, entry);
        }
        return Err(format!("{} keeps that already", project_name(&kept)));
    }
    let back = entry.moved().is_some();
    *entry.moved() = (!back).then(|| by.clone());
    let text = entry.text().to_owned();
    let mut touched = vec![from_key.clone(), to_key.clone()];
    match (to, &entry) {
        (Store::Personal, Moving::Rule(rule)) if rule.learned.agent.is_none() => {
            let same = words(&rule.text);
            let mut taken = Vec::new();
            for held in (state.held.values_mut()).filter(|held| held.store.is_project()) {
                let before = held.rules.len();
                let key = held.key.clone();
                held.rules.retain(|other| {
                    let keep = other.learned.agent.is_some() || words(&other.text) != same;
                    if !keep {
                        taken.push(Absorbed {
                            into: id,
                            key: key.clone(),
                            rule: other.clone(),
                        });
                    }
                    keep
                });
                if held.rules.len() != before {
                    touched.push(key);
                }
            }
            if let Some(target) = state.held.get_mut(&to_key) {
                target.absorbed.extend(taken);
            }
        }
        (Store::Project, _) => {
            let returning: Vec<Absorbed> = match state.held.get_mut(&from_key) {
                Some(from) => {
                    let (back, kept) =
                        (from.absorbed.drain(..)).partition(|taken| taken.into == id);
                    from.absorbed = kept;
                    back
                }
                None => Vec::new(),
            };
            for taken in returning {
                if let Some(held) = state.held.get_mut(&taken.key) {
                    held.rules.push(taken.rule);
                    touched.push(taken.key);
                }
            }
        }
        _ => {}
    }
    if let Some(target) = state.held.get_mut(&to_key) {
        put(target, entry);
    }
    touched.dedup();
    for key in &touched {
        core.memory().save(&mut state, key);
    }
    drop(state);
    tell_clients(core, false);
    tell_moved(core, project, id, &text, to, back, by);
    Ok(())
}

fn tell_moved(core: &Core, project: &str, id: u64, text: &str, to: Store, back: bool, by: Learned) {
    match back {
        true => crate::server::chief::rule_lines_back(core, project, |line| {
            line.moved && line.rule == id
        }),
        false => crate::server::chief::rule_moved(
            core,
            RuleChange {
                project: project.to_owned(),
                rule: id,
                text: crate::text::one_line(text),
                commit: None,
                was: None,
                by: by.person.unwrap_or_default(),
                restored: false,
                back_by: None,
                store: to,
                moved: true,
            },
        ),
    }
}

/// A move of entry `id` of the project at `project` whose own store another host keeps: the
/// half on this host. The entry as it was taken from `from`, and as it goes to the other store.
pub(crate) struct Taken {
    pub(crate) was: Moving,
    pub(crate) going: Moving,
}

/// Takes entry `id` out of `from` to move it to the other store, which another host keeps.
pub(crate) fn take_out(core: &Core, project: &str, id: u64, from: Store) -> Result<Taken, String> {
    let by = named(core, super::learned(core, None));
    let mut state = core.memory().state.locked();
    if state.promoting == Some(id) {
        return Err(format!("#{id} is being written into AGENTS.md"));
    }
    let key = state.key_for(project, from);
    let held = (state.held.get_mut(&key))
        .filter(|held| held.holds(id))
        .ok_or_else(|| format!("#{id} is not kept there"))?;
    let was = take(held, id).ok_or("the entry went while it was moved")?;
    held.not_now.remove(&id);
    let mut going = was.clone();
    let back = going.moved().is_some();
    *going.moved() = (!back).then_some(by);
    core.memory().save(&mut state, &key);
    drop(state);
    tell_clients(core, false);
    Ok(Taken { was, going })
}

/// Whether the store `to` of the project at `project` has room for entry `id`, kept now in the
/// other store, and does not keep the same words already.
pub(crate) fn can_take_in(core: &Core, project: &str, id: u64, to: Store) -> Result<(), String> {
    let state = core.memory().state.locked();
    let from = state.held.get(&state.key_for(project, to.other()));
    let text = from
        .and_then(|held| {
            (held
                .rules
                .iter()
                .find(|rule| rule.id == id)
                .map(|rule| rule.text.clone()))
            .or_else(|| {
                (held.knowledge.iter().find(|known| known.id == id)).map(|known| known.text.clone())
            })
        })
        .ok_or_else(|| format!("#{id} is not kept there"))?;
    let Some(target) = state.held.get(&state.key_for(project, to)) else {
        return Ok(());
    };
    if target.len() >= MAX_ENTRIES {
        return Err(format!(
            "That store already keeps {MAX_ENTRIES} rules and knowledge; retire some first"
        ));
    }
    let kept = (target.rules.iter().map(|rule| &rule.text))
        .chain(target.knowledge.iter().map(|known| &known.text))
        .any(|kept| kept.eq_ignore_ascii_case(&text));
    match kept {
        true => Err(format!("{} keeps that already", project_name(&target.key))),
        false => Ok(()),
    }
}

/// Puts `entry` into the store `to` of the project at `project`, as a move from the other store
/// that another host keeps.
pub(crate) fn put_in(core: &Core, project: &str, entry: Moving, to: Store) -> Result<(), String> {
    let mut state = core.memory().state.locked();
    let key = state.key_for(project, to);
    let target = holding(&mut state, &key, to)?;
    if target.len() >= MAX_ENTRIES {
        return Err(format!(
            "That store already keeps {MAX_ENTRIES} rules and knowledge; retire some first"
        ));
    }
    if entry.kept_in(target) || target.holds(entry.id()) {
        return Err(format!("{} keeps that already", project_name(&key)));
    }
    put(target, entry);
    core.memory().save(&mut state, &key);
    drop(state);
    tell_clients(core, false);
    Ok(())
}

/// Says in the chief's chat that a move with its other half on another host is done.
pub(crate) fn moved_across(core: &Core, project: &str, taken: &Taken, to: Store) {
    let mut going = taken.going.clone();
    let moved = going.moved().clone();
    let back = moved.is_none();
    let by = moved.unwrap_or_else(|| named(core, super::learned(core, None)));
    tell_moved(
        core,
        project,
        taken.going.id(),
        taken.going.text(),
        to,
        back,
        by,
    );
}

/// The person turned down the chief's offer to move entry `id`; it is not made again.
pub(super) fn not_now(core: &Core, project: &str, id: u64) -> Result<(), String> {
    let mut state = core.memory().state.locked();
    let key = state.key_holding(project, id)?;
    if let Some(held) = state.held.get_mut(&key) {
        held.not_now.insert(id);
    }
    core.memory().save(&mut state, &key);
    drop(state);
    tell_clients(core, false);
    Ok(())
}
