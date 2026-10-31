//! What a new agent is handed at the end of its task, in the same letter:
//! the project's rules, and once it holds a card, its goal's rules and
//! decisions, with only the knowledge its task and card name. Stale knowledge
//! stays out. A reviewer Auto started blind gets only what the person said,
//! nothing an agent added.

use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_wire::memory::Learned;

use super::check::{want, FRESH_MS};
use super::{goal_applies, Core};
use crate::server::board;

const MAX_RULES: usize = 30;
const MAX_KNOWLEDGE: usize = 12;
/// The newest decisions a brief carries; memory_list shows the rest.
const MAX_DECISIONS: usize = 20;

/// Whether `text` names `term` as a whole word, such as `ai-service` in
/// "move ai-service to ECS" but not `ai` in "details".
fn names(text: &str, term: &str) -> bool {
    let is_word = |char: char| char.is_alphanumeric() || char == '_';
    text.match_indices(term).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + term.len()..].chars().next();
        !before.is_some_and(is_word) && !after.is_some_and(is_word)
    })
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What Sikemux keeps for the agent, for after its task. Knowledge is picked
/// by the words of `task`, a lead's own, when given, and of the agent's card.
pub(crate) fn brief(core: &Core, agent_id: &str, task: Option<&str>) -> Option<String> {
    let project = core.names.get(agent_id)?.project;
    let blind = core.names.is_blind(agent_id);
    let from_person = |learned: &Learned| !blind || learned.agent.is_none();
    let goal = board::goal_of(core, agent_id);
    let mut read = task.unwrap_or_default().to_lowercase();
    if let Some(goal) = &goal {
        read.push('\n');
        read.push_str(&goal.live.to_lowercase());
    }
    let fresh_since = epoch_ms().saturating_sub(FRESH_MS);
    let (mut rules, mut knowledge, mut testing) = (Vec::new(), Vec::new(), Vec::new());
    {
        let state = core.memory.state.locked();
        for held in state.applying(&project) {
            for rule in &held.rules {
                if rules.len() < MAX_RULES
                    && from_person(&rule.learned)
                    && goal_applies(&held.folder, rule.goal, goal.as_ref())
                {
                    rules.push(format!("- #{} {}", rule.id, one_line(&rule.text)));
                }
            }
            for known in &held.knowledge {
                let named = known.about.is_empty()
                    || known
                        .about
                        .iter()
                        .any(|about| names(&read, &about.to_lowercase()));
                if knowledge.len() >= MAX_KNOWLEDGE
                    || known.stale.is_some()
                    || !named
                    || !from_person(&known.learned)
                {
                    continue;
                }
                knowledge.push(format!("- #{} {}", known.id, one_line(&known.text)));
                let fresh = known
                    .checked
                    .as_ref()
                    .is_some_and(|checked| checked.at >= fresh_since);
                if known.check.is_some() && !fresh {
                    testing.push(known.id);
                }
            }
        }
    }
    want(core, testing);
    let mut sections = Vec::new();
    if !rules.is_empty() {
        sections.push(format!("Rules:\n{}", rules.join("\n")));
    }
    if !knowledge.is_empty() {
        sections.push(format!("Knowledge:\n{}", knowledge.join("\n")));
    }
    if let Some(goal) = &goal {
        let mut decisions = board::decisions::of_goal(core, goal);
        decisions.retain(|decision| from_person(&decision.learned));
        let newest = decisions.len().saturating_sub(MAX_DECISIONS);
        let decisions: Vec<String> = decisions[newest..]
            .iter()
            .map(|decision| match &decision.why {
                Some(why) => format!(
                    "- #{} {} ({})",
                    decision.id,
                    one_line(&decision.text),
                    one_line(why)
                ),
                None => format!("- #{} {}", decision.id, one_line(&decision.text)),
            })
            .collect();
        if !decisions.is_empty() {
            sections.push(format!(
                "Decided for goal card #{}:\n{}",
                goal.card,
                decisions.join("\n")
            ));
        }
    }
    if sections.is_empty() {
        return None;
    }
    Some(format!(
        "---\n\n**What Sikemux keeps for this project.** Follow its rules and decisions. When something here is wrong, correct it: rules and knowledge with memory_update, decisions with board_decide. memory_list shows the rest and where each came from.\n\n{}",
        sections.join("\n\n")
    ))
}
