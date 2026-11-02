//! The rules the person keeps repeating, found once per project in what they
//! typed in its past sessions and offered to keep. Only their own messages
//! are read, without secrets or pasted text, and a model is asked once about
//! them, with no chat, tools or saved session, so nothing it does shows as an
//! agent or comes back as a past session. What it finds is an offer, never a
//! rule, until the person keeps it.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_wire::memory::{MemoryView, SeededRule, Seeding};

use super::{add, holding, run, tell_clients, view, Core, KeepSeededParams, NewMemory, State};
use crate::agent_catalog::AgentKind;
use crate::server::access::Peer;
use crate::server::board::goal::learned;
use crate::server::board::project_at;
use crate::server::saved;
use crate::text::clipped;

const MAX_RULES: usize = 20;
const MAX_RULE_CHARS: usize = 300;
/// A rule said in fewer sessions than this is not one the person repeats.
const MIN_SESSIONS: u32 = 2;
/// How long reading the past sessions may take.
const READ_BUDGET: Duration = Duration::from_secs(20);
/// The agents whose model is asked, in turn, until one is installed.
const ASKED: [AgentKind; 2] = [AgentKind::Claude, AgentKind::Codex];

const PROMPT: &str = "Below are messages one person typed to AI coding agents in past sessions of one project, grouped by session. Find the rules they keep repeating: standing instructions for how agents should work in this project, such as what to always or never do, how to test, commit, name or style things, and when to ask first. Leave out one-off tasks, questions, and anything about a single change. Keep only rules said in at least two different sessions. Write each as one short imperative sentence in plain words, with no names, secrets or quotes. Answer with only a JSON array of at most 20 objects, most repeated first, each {\"text\": the rule, \"sessions\": how many sessions said it}. Answer [] when there are none.";

/// A project's search of its past sessions, as its memory file keeps it: one
/// left reading by a core that stopped counts as having run.
pub(super) fn settled(seeding: Option<Seeding>) -> Option<Seeding> {
    match seeding {
        Some(Seeding::Reading { since }) => Some(Seeding::Ran { at: since, kept: 0 }),
        seeding => seeding,
    }
}

/// Starts the search of `project`'s past sessions, unless it ran before.
/// Called when the person starts the project's first goal.
pub(crate) fn seed_once(core: &Arc<Core>, project: &str) {
    let ran =
        (core.memory.state.locked().held.get(project)).is_some_and(|held| held.seeding.is_some());
    if ran {
        return;
    }
    if let Err(error) = begin(core, project) {
        eprintln!("sikemux core: {project}'s past sessions were not read for rules: {error}");
    }
}

/// Marks `project` as being read and starts reading it, unless it is already.
fn begin(core: &Arc<Core>, project: &str) -> Result<(), String> {
    {
        let mut state = core.memory.state.locked();
        let held = holding(&mut state, project)?;
        if matches!(held.seeding, Some(Seeding::Reading { .. })) {
            return Err("Sikemux is already reading this project's past sessions".into());
        }
        held.seeding = Some(Seeding::Reading { since: epoch_ms() });
        core.memory.save(&mut state, project);
    }
    tell_clients(core, false);
    let (core, project) = (core.clone(), project.to_owned());
    tokio::spawn(async move {
        let (reading, folder) = (core.clone(), project.clone());
        let found = run(move || Ok(find(&reading, &folder))).await;
        let rules = found.unwrap_or_else(|error| {
            eprintln!(
                "sikemux core: the rules in {project}'s past sessions were not found: {error}"
            );
            Vec::new()
        });
        end(&core, &project, rules);
    });
    Ok(())
}

/// The rules the person repeats in `project`'s past sessions, as the model
/// found them, without those kept already. None when nothing could be read
/// or no model answered in a way that can be read.
fn find(core: &Core, project: &str) -> Vec<SeededRule> {
    let sessions = saved::typed_in(core, project, READ_BUDGET);
    if sessions.len() < MIN_SESSIONS as usize {
        return Vec::new();
    }
    let mut prompt = PROMPT.to_owned();
    for (at, messages) in sessions.iter().enumerate() {
        prompt.push_str(&format!("\n\n## Session {}\n", at + 1));
        for message in messages {
            prompt.push_str(&format!("- {message}\n"));
        }
    }
    let dir = asked_in(core);
    let answer = ASKED.iter().find_map(|agent| {
        let ask = agent.ops().ask_once()?;
        let model = agent.entry().commit_models.first()?;
        match ask(&dir.to_string_lossy(), model, &prompt) {
            Ok(answer) => Some(answer),
            Err(error) => {
                eprintln!(
                    "sikemux core: {} found no rules: {error}",
                    agent.entry().label
                );
                None
            }
        }
    });
    let kept: Vec<String> = (core.memory.state.locked().held.get(project))
        .map(|held| {
            held.rules
                .iter()
                .map(|rule| rule.text.to_lowercase())
                .collect()
        })
        .unwrap_or_default();
    answer
        .map(|answer| rules_in(&answer, &kept))
        .unwrap_or_default()
}

/// Where the model is asked: the core's own folder, outside every project.
fn asked_in(core: &Core) -> PathBuf {
    (core.memory.folder.clone()).unwrap_or_else(std::env::temp_dir)
}

#[derive(Deserialize)]
struct Found {
    text: String,
    #[serde(default)]
    sessions: u32,
}

/// The rules in the model's answer, a JSON list, bounded and without those
/// in `kept` or said too seldom.
fn rules_in(answer: &str, kept: &[String]) -> Vec<SeededRule> {
    let list = answer
        .find('[')
        .zip(answer.rfind(']'))
        .filter(|(start, end)| start < end)
        .and_then(|(start, end)| serde_json::from_str::<Vec<Found>>(&answer[start..=end]).ok())
        .unwrap_or_default();
    let mut rules: Vec<SeededRule> = Vec::new();
    for found in list {
        let text = clipped(
            &found.text.split_whitespace().collect::<Vec<_>>().join(" "),
            MAX_RULE_CHARS,
        );
        let known = |other: &str| other.eq_ignore_ascii_case(&text);
        if text.is_empty()
            || found.sessions < MIN_SESSIONS
            || kept.iter().any(|kept| known(kept))
            || rules.iter().any(|rule| known(&rule.text))
        {
            continue;
        }
        rules.push(SeededRule {
            id: rules.len() as u64 + 1,
            text,
            sessions: found.sessions,
        });
        if rules.len() >= MAX_RULES {
            break;
        }
    }
    rules
}

/// Keeps what the search of `project` found as an offer, or that it ran when
/// it found nothing.
fn end(core: &Core, project: &str, rules: Vec<SeededRule>) {
    let mut state = core.memory.state.locked();
    let Some(held) = state.held.get_mut(project) else {
        return;
    };
    let at = epoch_ms();
    held.seeding = Some(match rules.is_empty() {
        true => Seeding::Ran { at, kept: 0 },
        false => Seeding::Offered { rules, at },
    });
    core.memory.save(&mut state, project);
    drop(state);
    tell_clients(core, true);
}

/// The rules offered in `project`, while they are.
fn offered(state: &State, project: &str) -> Option<Vec<SeededRule>> {
    match &state.held.get(project)?.seeding {
        Some(Seeding::Offered { rules, .. }) => Some(rules.clone()),
        _ => None,
    }
}

/// `keepSeededRules`: keeps the offered rules the person ticked as the
/// project's rules, and ends the offer.
pub(crate) async fn keep(
    core: &Arc<Core>,
    peer: Peer,
    params: KeepSeededParams,
) -> Result<MemoryView, String> {
    let project = project_at(core, &params.project)?;
    let core = core.clone();
    run(move || {
        let rules = offered(&core.memory.state.locked(), &project)
            .ok_or("Sikemux offers no rules from past sessions in this project now")?;
        if let Some(id) = (params.keep.iter()).find(|id| !rules.iter().any(|rule| rule.id == **id))
        {
            return Err(format!("No rule offered is #{id}"));
        }
        let mut kept = 0;
        for rule in rules.iter().filter(|rule| params.keep.contains(&rule.id)) {
            let new = NewMemory::Rule {
                text: rule.text.clone(),
                goal: None,
                sensitive: false,
                folder: None,
            };
            match add(&core, &project, new, learned(&core, None)) {
                Ok(_) => kept += 1,
                Err(error) => {
                    eprintln!("sikemux core: a rule from past sessions was not kept: {error}")
                }
            }
        }
        let mut state = core.memory.state.locked();
        if let Some(held) = state.held.get_mut(&project) {
            held.seeding = Some(Seeding::Ran {
                at: epoch_ms(),
                kept,
            });
        }
        core.memory.save(&mut state, &project);
        drop(state);
        tell_clients(&core, true);
        Ok(view(&core, &project, &peer))
    })
    .await
}

/// `seedRules`: looks through the project's past sessions for rules again.
pub(crate) async fn again(
    core: &Arc<Core>,
    peer: Peer,
    params: super::MemoryParams,
) -> Result<MemoryView, String> {
    let project = project_at(core, &params.project)?;
    begin(core, &project)?;
    let core = core.clone();
    run(move || Ok(view(&core, &project, &peer))).await
}

/// A project's offer, for the inbox.
pub(crate) struct Offer {
    pub project: String,
    pub rules: Vec<SeededRule>,
    pub at: u64,
}

pub(crate) fn offers(core: &Core) -> Vec<Offer> {
    let state = core.memory.state.locked();
    (state.held.values())
        .filter_map(|held| match &held.seeding {
            Some(Seeding::Offered { rules, at }) => Some(Offer {
                project: held.folder.clone(),
                rules: rules.clone(),
                at: *at,
            }),
            _ => None,
        })
        .collect()
}
