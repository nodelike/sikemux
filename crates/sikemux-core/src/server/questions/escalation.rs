use super::*;

/// The agent's turn ended, so a question it was handed and did not answer
/// starts its time to be passed over.
pub(crate) fn turn_ended(core: &Core, agent_id: &str) {
    let questions = &core.harness().questions;
    let waiting: Vec<String> = questions
        .state
        .locked()
        .open
        .iter()
        .filter(|question| question.lead_id() == Some(agent_id))
        .map(|question| question.id.clone())
        .collect();
    let read: HashSet<String> = waiting
        .into_iter()
        .filter(|id| !messages::is_waiting(core, agent_id, id))
        .collect();
    if read.is_empty() {
        return;
    }
    let now = epoch_ms();
    let mut state = questions.state.locked();
    for question in &mut state.open {
        if let Stage::Lead { ended_at, .. } = &mut question.stage {
            if read.contains(&question.id) && ended_at.is_none() {
                *ended_at = Some(now);
            }
        }
    }
    questions.changed(&mut state);
}

/// The chat's turn ended by itself: what its reply offers, and what it says it waits on.
pub(crate) fn reply_read(core: &Arc<Core>, agent_id: &str) {
    offers_in_reply(core, agent_id);
    checks_in_reply(core, agent_id);
    waits_in_reply(core, agent_id);
}

/// Whether the reply to a repeating check's firing says the agent still waits on it. A firing
/// still on its way in is for the turn it starts to answer.
fn checks_in_reply(core: &Core, agent_id: &str) {
    let Some(chat) = core.chats().get(agent_id) else {
        return;
    };
    if core.harness().mailbox.has_waiting(agent_id) || chat.feed.has_queued() {
        return;
    }
    let waits = (chat.feed.last_answer()).is_some_and(|reply| waits_in(&reply));
    core.triggers().turn_ended(agent_id, waits);
}

/// An offer of more work in a reply, which a yes or a no answers, sends the agent back when a
/// project rule covers it, and otherwise only shows as how the turn left things. An agent with a
/// lead, here or on a linked host, answers to them instead, and one with a message still on its
/// way is not done.
fn offers_in_reply(core: &Arc<Core>, agent_id: &str) {
    let Some(chat) = core.chats().get(agent_id) else {
        return;
    };
    let named = core.names().get(agent_id);
    if named.as_ref().is_some_and(|named| named.parent.is_some())
        || core.harness().mailbox.has_waiting(agent_id)
        || chat.feed.has_queued()
    {
        return;
    }
    let Some(reply) = chat.feed.last_answer() else {
        return;
    };
    let offers = (asked_in(&reply).into_iter())
        .filter(|ask| ask.polar)
        .map(|offer| offer.text);
    let project = (named.map(|named| named.project))
        .unwrap_or_else(|| core.workspaces().project_of(&chat.launch.cwd));
    follow_rule(core, agent_id, &project, offers);
}

/// A worker whose reply says it waits, with nothing set to wake it, is sent back once to set a
/// wake or say what is left for its lead. Ending that way again makes its report urgent.
fn waits_in_reply(core: &Arc<Core>, agent_id: &str) {
    let caught = unwoken(core, agent_id);
    let mut state = core.harness().questions.state.locked();
    let Some(to) = caught else {
        state.unwoken.remove(agent_id);
        return;
    };
    if state.unwoken.insert(agent_id.to_owned(), true).is_some() {
        return;
    }
    state.unwoken.insert(agent_id.to_owned(), false);
    drop(state);
    let lead = (core.names().name(&to)).map_or("your lead".to_owned(), |name| format!("@{name}"));
    let text = format!(
        "Your turn ended saying you wait, but nothing is set to wake you, so you would sit until someone writes to you. If you wait on something, set trigger_add for it now: process, agent or ci when it is one, else every with a check that prints once it is ready, and a timeout. A repeating check that already fired is no wait unless the turn it prompted ended saying you still wait, so remove it and set it again. If it is not yours to wait on, say plainly what is left for {lead} and end your turn."
    );
    let (core, agent_id) = (core.clone(), agent_id.to_owned());
    tokio::spawn(async move {
        let why = "Sent by Sikemux itself because your turn ended waiting with nothing set to wake you, not by the person.";
        if let Err(error) = messages::send_back(&core, agent_id, text, why).await {
            eprintln!("sikemux core: an agent waiting on nothing was not sent back: {error}");
        }
    });
}

/// Whoever hears about a worker whose reply says it waits while nothing is set to wake it: its
/// lead while open, else the project's chief.
pub(super) fn unwoken(core: &Core, agent_id: &str) -> Option<String> {
    core.names().reports_to(agent_id)?;
    let to = crate::server::roster::hears_of(core, agent_id)
        .agent()?
        .to_owned();
    let chat = core.chats().get(agent_id)?;
    let reply = chat.feed.last_answer()?;
    if !waits_in(&reply) || asks_in(&reply) {
        return None;
    }
    let now = core.agent_now(agent_id);
    let woken = now.in_background()
        || chat.feed.has_queued()
        || now.waits_on_something()
        || now.workers_busy(agent_id)
        || crate::server::turn_state::set_a_wake(core, agent_id);
    (!woken).then_some(to)
}

/// Whether the agent's last turn ended waiting on nothing set to wake it, and Sikemux sent it
/// back for that, in place of telling its lead.
pub(crate) fn sent_back_unwoken(core: &Core, agent_id: &str) -> bool {
    (core.harness().questions.state.locked().unwoken).get(agent_id) == Some(&false)
}

/// Whether the agent's last turn ended waiting on nothing set to wake it.
pub(crate) fn waits_unwoken(core: &Core, agent_id: &str) -> bool {
    (core.harness().questions.state.locked().unwoken).contains_key(agent_id)
}

/// Whether an agent's reply asks the person anything, in its own words.
pub(crate) fn asks_in(reply: &str) -> bool {
    !asked_in(reply).is_empty()
}

/// Whether the last paragraph of an agent's reply says it waits on something, or goes on once
/// something happens: "Still waiting on CI.", "I'll land once clippy passes."
pub(crate) fn waits_in(reply: &str) -> bool {
    let reply = reply.replace('\u{2019}', "'").to_lowercase();
    let last = last_paragraph(&reply)
        .replace(". ", "\n")
        .replace("! ", "\n");
    last.lines()
        .map(|sentence| sentence.trim_start_matches(['-', '*', '#', ' ', '>']))
        .any(says_it_waits)
}

fn last_paragraph(reply: &str) -> &str {
    reply.trim().rsplit("\n\n").next().unwrap_or_default()
}

fn says_it_waits(sentence: &str) -> bool {
    if sentence.starts_with("if ") {
        return false;
    }
    let padded = format!(" {} ", sentence.replace([',', ';', ':', '.', '*'], " "));
    let has = |words: &[&str]| words.iter().any(|word| padded.contains(word));
    let waiting_on = sentence
        .trim_start_matches("still ")
        .starts_with("waiting ")
        && has(&[" waiting for ", " waiting on ", " waiting until "]);
    let i_wait = has(&[
        " i'm waiting ",
        " i am waiting ",
        " i'm still waiting ",
        " i am still waiting ",
    ]);
    let polite_handoff = has(&[
        " you ",
        " your ",
        " you're ",
        " needed ",
        " ready ",
        " asked ",
        " let me know ",
        " happy to ",
    ]);
    let goes_on_once = !polite_handoff
        && has(&[" i'll ", " i will "])
        && has(&[" once ", " when ", " as soon as ", " after ", " until "]);
    let still_going = has(&[
        " still compiling ",
        " still building ",
        " still running ",
        " still in progress ",
        " still queued ",
    ]) && !sentence.contains('@')
        && !sentence.contains("n't")
        && !has(&[" no ", " nothing ", " not "]);
    waiting_on || i_wait || goes_on_once || still_going
}

/// The first offer of more work in an agent's reply, which a yes or a no answers.
pub(crate) fn offer_in(reply: &str) -> Option<String> {
    let mut asks = asked_in(reply).into_iter();
    asks.find_map(|ask| ask.polar.then_some(ask.text))
}

/// Sends the agent back to the first of its offers a project rule is about, for the rule to say
/// whether it does the work or leaves it for the person. A rule sends it back once in a row, so
/// offering it again leaves it as state.
fn follow_rule(
    core: &Arc<Core>,
    agent_id: &str,
    project: &str,
    offers: impl Iterator<Item = String>,
) {
    let covered = offers.into_iter().find_map(|offer| {
        let rule = crate::server::memory::rule_about(core, project, &offered_work(&offer)?)?;
        Some((offer, rule))
    });
    let Some((offer, rule)) = covered else {
        let followed = &mut core.harness().questions.state.locked().followed_rules;
        followed.remove(agent_id);
        return;
    };
    let mut state = core.harness().questions.state.locked();
    if state.followed_rules.get(agent_id) == Some(&rule) {
        return;
    }
    state
        .followed_rules
        .insert(agent_id.to_owned(), rule.clone());
    drop(state);
    let (core, agent_id) = (core.clone(), agent_id.to_owned());
    tokio::spawn(async move {
        let text = format!(
            "You ended your turn offering: {offer}\n\nA rule of this project is about that: {rule}\n\nThe rule decides, not this note. If it tells you to do the work, do it. If it says to wait for the person's word, or not to do it, leave the work as it is and end your turn without offering it again: the person sees how you left it on your row and takes it up in your chat. What leaves this machine still stops for them."
        );
        let why = "Sent by Sikemux itself over a rule of this project, not by the person.";
        if let Err(error) = messages::send_back(&core, agent_id, text, why).await {
            eprintln!("sikemux core: an agent was not sent back over a rule: {error}");
        }
    });
}

pub(crate) fn prune_agents(core: &Core, active: &HashSet<String>) {
    let mut state = core.harness().questions.state.locked();
    state
        .followed_rules
        .retain(|agent, _| active.contains(agent));
    state.unwoken.retain(|agent, _| active.contains(agent));
}

/// Moves on questions their lead left, drops those of agents that are gone,
/// and tells the window and phones what changed. Runs when questions or the
/// agents change, and again when a lead's time to answer runs out.
pub(crate) fn settle(core: &Arc<Core>) {
    let unpicked = std::mem::take(&mut core.harness().questions.state.locked().unpicked);
    for (asker, note) in unpicked {
        let core = core.clone();
        tokio::spawn(async move {
            let _ = messages::work_from_sikemux(&core, asker, note, "about a call of yours").await;
        });
    }
    let (askers, leads): (HashSet<String>, HashSet<String>) = {
        let state = core.harness().questions.state.locked();
        if state.dirty {
            core.boards().wake.now();
        }
        if state.open.is_empty()
            && state.answered.is_empty()
            && state.told_phones.is_empty()
            && state.shown_devices.is_empty()
            && !state.dirty
        {
            drop(state);
            tell_calls(core);
            tell_window(core);
            return;
        }
        (
            state
                .open
                .iter()
                .chain(state.answered.iter().map(|answered| &answered.question))
                .map(|question| question.agent_id.clone())
                .collect(),
            state
                .open
                .iter()
                .filter_map(|question| question.lead_id().map(str::to_owned))
                .collect(),
        )
    };
    let gone: HashSet<String> = askers
        .into_iter()
        .filter(|agent_id| gone(core, agent_id))
        .collect();
    let leads: HashMap<String, (bool, bool)> = leads
        .into_iter()
        .map(|lead| {
            let state = (
                reachable(core, &lead),
                core.agent_now(&lead).maybe_mid_turn(),
            );
            (lead, state)
        })
        .collect();
    let unread: HashSet<String> = {
        let state = core.harness().questions.state.locked();
        state
            .open
            .iter()
            .filter_map(|question| {
                let lead = question.lead_id()?;
                messages::is_waiting(core, lead, &question.id).then(|| question.id.clone())
            })
            .collect()
    };
    let now = epoch_ms();
    let mut withdrawn = Vec::new();
    let mut next = None;
    {
        let mut state = core.harness().questions.state.locked();
        let before = state.open.len();
        state
            .open
            .retain(|question| !gone.contains(&question.agent_id));
        let answered = state.answered.len();
        state
            .answered
            .retain(|answered| !gone.contains(&answered.question.agent_id));
        let mut changed = state.open.len() != before || state.answered.len() != answered;
        let held: HashSet<String> = (state.open.iter())
            .flat_map(|question| question.holds.iter().cloned())
            .collect();
        for question in &mut state.open {
            let Stage::Lead {
                lead_id,
                lead_name,
                ended_at,
            } = &mut question.stage
            else {
                continue;
            };
            if held.contains(&question.id) {
                continue;
            }
            let (reachable, busy) = leads
                .get(lead_id.as_str())
                .copied()
                .unwrap_or((false, false));
            let unread = unread.contains(&question.id);
            let passed_over =
                ended_at.is_some_and(|at| now.saturating_sub(at) >= LEAD_GRACE_MS) && !unread;
            if busy {
                *ended_at = None;
            } else if !reachable || passed_over {
                if unread {
                    withdrawn.push((lead_id.clone(), question.id.clone()));
                }
                question.stage = Stage::Person {
                    lead: (!unread).then(|| (lead_id.clone(), lead_name.clone())),
                };
                changed = true;
            } else if let Some(at) = ended_at.filter(|_| !unread) {
                next = sooner(next, Some(at + LEAD_GRACE_MS));
            }
        }
        if changed {
            core.harness().questions.changed(&mut state);
            core.boards().wake.now();
        }
    }
    for (lead, question) in withdrawn {
        messages::withdraw(core, &lead, &question);
    }
    relay(core);
    if let Some(next) = next {
        core.harness().questions.wake.at(next);
    }
    tell_calls(core);
    tell_phones(core);
    tell_window(core);
}

/// A question that waits on the person, as phones are told of it.
#[derive(Clone)]
struct Asking {
    agent_id: String,
    name: String,
    text: String,
    at: u64,
}

impl Asking {
    fn new(agent_id: &str, name: &str, text: &str, at: u64) -> Self {
        Self {
            agent_id: agent_id.to_owned(),
            name: name.to_owned(),
            text: text.to_owned(),
            at,
        }
    }
}

/// Tells phones of each question that now waits on the person, and of each
/// that no longer does. An agent whose questions to the person changed counts
/// as active, which changes the view devices are sent, so they read the
/// questions again.
pub(super) fn tell_phones(core: &Core) {
    let (asked, cleared, changed) = {
        let mut state = core.harness().questions.state.locked();
        let shown: HashMap<String, String> = state
            .open
            .iter()
            .filter(|question| question.listed_for_devices())
            .map(|question| (question.id.clone(), question.agent_id.clone()))
            .collect();
        let changed: HashSet<String> = shown
            .iter()
            .filter(|(id, _)| !state.shown_devices.contains_key(*id))
            .chain(
                state
                    .shown_devices
                    .iter()
                    .filter(|(id, _)| !shown.contains_key(*id)),
            )
            .map(|(_, agent_id)| agent_id.clone())
            .collect();
        state.shown_devices = shown;
        let now: HashMap<String, Asking> = (state.open.iter())
            .filter(|question| question.listed_for_devices())
            .filter(|question| question.kind != Some(CallKind::Look))
            .map(|question| {
                let asking = Asking::new(
                    &question.agent_id,
                    &question.name,
                    &question.text,
                    question.at,
                );
                (question.id.clone(), asking)
            })
            .collect();
        let asked: Vec<(String, Asking)> = (now.iter())
            .filter(|(id, _)| !state.told_phones.contains_key(*id))
            .map(|(id, asking)| (id.clone(), asking.clone()))
            .collect();
        let cleared: Vec<(String, String)> = state
            .told_phones
            .iter()
            .filter(|(id, _)| !now.contains_key(*id))
            .map(|(id, agent_id)| (id.clone(), agent_id.clone()))
            .collect();
        state.told_phones = now
            .into_iter()
            .map(|(id, asking)| (id, asking.agent_id))
            .collect();
        (asked, cleared, changed)
    };
    for agent_id in changed {
        core.seen().active(&agent_id);
    }
    for (id, asking) in asked {
        core.note_activity(&asking.agent_id, Activity::NeedsYou);
        core.notify().send(Signal::Question {
            id,
            agent_id: asking.agent_id,
            name: asking.name,
            text: asking.text,
            at: asking.at,
        });
    }
    for (id, agent_id) in cleared {
        core.notify().send(Signal::QuestionCleared { id, agent_id });
    }
}

/// The questions put to the person, as calls. A call nobody is left to hear the answer of
/// closes.
pub(super) fn tell_calls(core: &Core) {
    let wanted = {
        let state = core.harness().questions.state.locked();
        (state.open.iter())
            .filter(|question| question.listed_for_devices())
            .map(|question| (question.call(), question.allows()))
            .map(|(call, yes)| person_calls::Wanted {
                source: person_calls::Source::Question(call.id.clone()),
                call,
                key: None,
                yes,
            })
            .collect()
    };
    person_calls::sync_questions(core, wanted);
    person_calls::close_left(core, |agent_id| gone(core, agent_id));
}

/// Sends the window the open questions when they changed, or when a window
/// opened that has not had them.
pub(super) fn tell_window(core: &Arc<Core>) {
    let window = core.window().client_id();
    let (open, answered) = {
        let mut state = core.harness().questions.state.locked();
        if !state.dirty && state.told_window == window {
            return;
        }
        if state.dirty {
            core.changed(What::View);
            core.changed(What::Read(Topic::Questions));
        }
        state.dirty = false;
        state.told_window = window;
        if window.is_none() {
            return;
        }
        (state.open.clone(), state.answered.clone())
    };
    let views = views_of(core, open, answered);
    let request = json!({
        "id": "agent-questions",
        "project": "/",
        "agentId": null,
        "method": "agent.questions",
        "params": { "questions": views },
    });
    let Ok(call) = core.window().call(
        WindowCall::Harness { request },
        "show agents' questions",
        false,
        None,
    ) else {
        return;
    };
    let id = call.id;
    let core = core.clone();
    tokio::spawn(async move {
        if tokio::time::timeout(WINDOW_ANSWER_WAIT, call.answer)
            .await
            .is_err()
        {
            core.window().forget(id);
        }
    });
}

/// The note on `agent`'s open calls that goes with the person's message to its chat.
pub(crate) fn open_calls(core: &Core, agent: &str) -> Option<sikemux_wire::protocol::ChatContext> {
    let text = core.harness().questions.open_calls_note(agent)?;
    Some(sikemux_wire::protocol::ChatContext {
        uri: format!("{}open-calls", sikemux_wire::protocol::FOR_AGENT_URI),
        title: "Open calls".into(),
        text,
    })
}
