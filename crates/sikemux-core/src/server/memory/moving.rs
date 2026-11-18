//! The person moves a rule or knowledge between their own store and the project's, keeping its
//! id, and the chief's chat says so with Undo, which moves it back. The chief offers the move on
//! a rule of the person's that names something of this repository, and on a project's rule the
//! person keeps for another project too.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;
use sikemux_view::chat::RuleChange;
use sikemux_view::memory::{Knowledge, Learned, MemoryCheck, MemoryRule, MoveOffer, Moved, Store};

use super::{holding, tell_clients, Core, Held, Kept, State, MAX_ENTRIES};
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

    fn sensitive(&self) -> bool {
        match self {
            Moving::Rule(rule) => rule.sensitive,
            Moving::Knowledge(known) => known.sensitive,
        }
    }

    fn moved(&mut self) -> &mut Option<Moved> {
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

/// Why `entry` cannot go to `to`: knowledge whose check reads a path inside the project would
/// read it from the person's home once it is theirs.
fn stays(entry: &Moving, to: Store) -> Result<(), String> {
    match entry {
        Moving::Knowledge(known) if to == Store::Personal => match &known.check {
            Some(MemoryCheck::PathExists { path }) if !Path::new(path).is_absolute() => {
                Err(format!(
                    "#{} is checked by {path} inside this project, so it stays the project's",
                    known.id
                ))
            }
            _ => Ok(()),
        },
        _ => Ok(()),
    }
}

/// Whether `moved` took the entry from the store keyed `to`, or kept under a key `to` had before,
/// so moving it there is moving it back.
pub(super) fn back(state: &State, moved: &Option<Moved>, to: &str) -> bool {
    moved.as_ref().is_some_and(|moved| {
        moved.from == to || (state.held.get(to)).is_some_and(|held| held.was.contains(&moved.from))
    })
}

/// Waits for the stores' files, and says when one could not be written.
fn written(files: Vec<Option<Arc<Kept>>>) -> Result<(), String> {
    for file in files.into_iter().flatten() {
        file.flush();
        if let Some(error) = file.unsaved() {
            return Err(format!("Sikemux could not save the move: {error}"));
        }
    }
    Ok(())
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

/// A project as the person knows it: the name of a folder it is open in here, else the last part
/// of its store's key.
pub(super) fn project_name(state: &State, key: &str) -> String {
    let folder = (state.keys.iter())
        .filter(|(_, kept)| *kept == key)
        .map(|(folder, _)| folder.as_str())
        .min();
    let last = folder
        .unwrap_or(key)
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
                    .map(|rule| (words(&rule.text), project_name(state, &held.key)))
            })
            .collect();
        Self {
            folder: Path::new(project),
            elsewhere,
        }
    }

    pub(super) fn for_rule(&self, held: &Held, rule: &MemoryRule) -> Option<MoveOffer> {
        if rule.learned.agent.is_some() || rule.moved.is_some() || held.not_now.contains(&rule.id) {
            return None;
        }
        match held.store {
            Store::Personal => names_here(&rule.text, self.folder).then_some(MoveOffer::AboutHere),
            Store::Project => {
                (self.elsewhere.get(&words(&rule.text))).map(|elsewhere| MoveOffer::SaidElsewhere {
                    elsewhere: elsewhere.clone(),
                })
            }
            Store::Org => None,
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

pub(super) fn rekeyed(held: &mut Held, old: &str, new: &str) -> bool {
    let mut changed = false;
    for taken in held.absorbed.iter_mut().filter(|taken| taken.key == old) {
        taken.key = new.to_owned();
        changed = true;
    }
    changed
}

/// Points what `held` keeps about entry `old` at its new number, and remembers the change for
/// whoever still names it by the old one.
pub(super) fn renumbered(held: &mut Held, old: u64, new: u64) {
    for taken in held.absorbed.iter_mut().filter(|taken| taken.into == old) {
        taken.into = new;
    }
    for now in held.renumbered.values_mut().filter(|now| **now == old) {
        *now = new;
    }
    held.renumbered.insert(old, new);
    if held.not_now.remove(&old) {
        held.not_now.insert(new);
    }
    if held.waiting.remove(&old) {
        held.waiting.insert(new);
    }
    if let Some(by) = held.dropped.remove(&old) {
        held.dropped.insert(new, by);
    }
    if let Some(why) = held.refused.remove(&old) {
        held.refused.insert(new, why);
    }
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
    if to_key.is_empty() {
        return Err(super::NOT_SHARED.into());
    }
    if by.agent.is_none() {
        if let Some(listed) = super::waiting::listed(&mut state, project, &from_key, id, to) {
            core.memory().save(&mut state, &from_key);
            drop(state);
            super::tell_clients(core, false);
            listed?;
            core.memory().org_sync.notify_one();
            return Ok(());
        }
    }
    if from_key == to_key {
        return Err(format!("#{id} is kept there already"));
    }
    let by_agent = by.agent.is_some();
    let refused = super::org::refused(&state, project, &from_key, by_agent)
        .or_else(|| super::org::refused(&state, project, &to_key, by_agent));
    if let Some(refused) = refused {
        return Err(refused);
    }
    let across_org = to == Store::Org
        || state
            .held
            .get(&from_key)
            .is_some_and(|held| held.store == Store::Org);
    let target = holding(&mut state, &to_key, to)?;
    if target.len() >= MAX_ENTRIES {
        return Err(format!(
            "That store already keeps {MAX_ENTRIES} rules and knowledge; retire some first"
        ));
    }
    let from = (state.held.get_mut(&from_key)).ok_or("the store went while it was changed")?;
    let mut entry = take(from, id).ok_or("the entry went while it was moved")?;
    let refused = stays(&entry, to).err().or_else(|| {
        (state.held.get(&to_key))
            .filter(|target| entry.kept_in(target))
            .map(|target| format!("{} keeps that already", project_name(&state, &target.key)))
    });
    let refused = refused.or_else(|| {
        let mut trial = state.held.get(&to_key)?.clone();
        put(&mut trial, entry.clone());
        super::org::too_big(&trial)
    });
    let refused = refused.or_else(|| {
        let from = state.held.get(&from_key)?;
        (from.absorbed.iter())
            .filter(|taken| taken.into == id && taken.key != to_key)
            .find(|taken| !state.held.contains_key(&taken.key))
            .map(|_| "A project it took the same rule from is gone, so it can't go back".to_owned())
    });
    if let Some(refused) = refused {
        if let Some(from) = state.held.get_mut(&from_key) {
            put(from, entry);
        }
        return Err(refused);
    }
    if let Some(from) = state.held.get_mut(&from_key) {
        from.not_now.remove(&id);
        from.refused.remove(&id);
    }
    let back = back(&state, entry.moved(), &to_key);
    *entry.moved() = (!back).then(|| Moved {
        by: by.clone(),
        from: from_key.clone(),
    });
    let sensitive = entry.sensitive();
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
            for taken in returning.into_iter().filter(|taken| taken.key != to_key) {
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
    touched.sort();
    touched.dedup();
    let files = (touched.iter())
        .map(|key| core.memory().save(&mut state, key))
        .collect();
    drop(state);
    tell_clients(core, false);
    written(files)?;
    if !sensitive && !across_org {
        tell_moved(core, project, id, &text, to, back);
    }
    Ok(())
}

fn tell_moved(core: &Core, project: &str, id: u64, text: &str, to: Store, back: bool) {
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
                by: String::new(),
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
    let to_key = state.key_for(project, from.other());
    let held = (state.held.get_mut(&key))
        .filter(|held| held.holds(id))
        .ok_or_else(|| format!("#{id} is not kept there"))?;
    let was = take(held, id).ok_or("the entry went while it was moved")?;
    if let Err(refused) = stays(&was, from.other()) {
        put(held, was);
        return Err(refused);
    }
    held.not_now.remove(&id);
    let mut going = was.clone();
    let back = back(&state, going.moved(), &to_key);
    *going.moved() = (!back).then(|| Moved {
        by,
        from: key.clone(),
    });
    let file = core.memory().save(&mut state, &key);
    drop(state);
    tell_clients(core, false);
    written(vec![file])?;
    Ok(Taken { was, going })
}

/// Whether the store `to` of the project at `project` has room for entry `id`, kept now in the
/// other store, and does not keep the same words already.
pub(crate) fn can_take_in(core: &Core, project: &str, id: u64, to: Store) -> Result<(), String> {
    let state = core.memory().state.locked();
    let from = state.held.get(&state.key_for(project, to.other()));
    let found = from
        .and_then(|held| {
            (held.rules.iter().find(|rule| rule.id == id))
                .map(|rule| Moving::Rule(Box::new(rule.clone())))
                .or_else(|| {
                    (held.knowledge.iter().find(|known| known.id == id))
                        .map(|known| Moving::Knowledge(Box::new(known.clone())))
                })
        })
        .ok_or_else(|| format!("#{id} is not kept there"))?;
    stays(&found, to)?;
    let text = found.text();
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
        .any(|kept| kept.eq_ignore_ascii_case(text));
    match kept {
        true => Err(format!(
            "{} keeps that already",
            project_name(&state, &target.key)
        )),
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
        return Err(format!("{} keeps that already", project_name(&state, &key)));
    }
    put(target, entry);
    let file = core.memory().save(&mut state, &key);
    drop(state);
    tell_clients(core, false);
    written(vec![file])
}

/// Says in the chief's chat that a move with its other half on another host is done.
pub(crate) fn moved_across(core: &Core, project: &str, taken: &Taken, to: Store) {
    let mut going = taken.going.clone();
    if !going.sensitive() {
        let back = going.moved().is_none();
        tell_moved(core, project, going.id(), going.text(), to, back);
    }
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
