//! Questions that travel between linked hosts: a lead's on another host hears of what its worker
//! here asked the person, and answers come back.

use super::*;

/// Puts each question now for the person, whose asker's team is led from a
/// linked host, to the person there too, and tells that host of those no longer open.
pub(super) fn relay(core: &Core) {
    let closed: Vec<(String, String)> = {
        let mut state = core.harness.questions.state.locked();
        let state = &mut *state;
        for question in &state.open {
            if let Some(host) = &question.relayed {
                state.relayed_to.insert(question.id.clone(), host.clone());
            }
        }
        let open = &state.open;
        let mut closed = Vec::new();
        state.relayed_to.retain(|id, host| {
            let still = open.iter().any(|question| &question.id == id);
            if !still {
                closed.push((host.clone(), id.clone()));
            }
            still
        });
        closed
    };
    for (host, id) in closed {
        crate::server::hosts::route::relay_closed(core, &host, &id);
    }
    let candidates: Vec<String> = core
        .harness
        .questions
        .state
        .locked()
        .open
        .iter()
        .filter(|question| {
            matches!(question.stage, Stage::Person { .. })
                && question.relayed.is_none()
                && question.from_host.is_none()
        })
        .map(|question| question.agent_id.clone())
        .collect();
    if candidates.is_empty() {
        return;
    }
    let hosts: HashMap<String, String> = candidates
        .into_iter()
        .filter_map(|agent_id| {
            let host = crate::server::hosts::route::lead_host(core, &agent_id)?;
            Some((agent_id, host))
        })
        .collect();
    if hosts.is_empty() {
        return;
    }
    let mut relayed = Vec::new();
    {
        let mut state = core.harness.questions.state.locked();
        for question in &mut state.open {
            let Some(host) = hosts.get(&question.agent_id) else {
                continue;
            };
            if !matches!(question.stage, Stage::Person { .. }) || question.relayed.is_some() {
                continue;
            }
            question.relayed = Some(host.clone());
            relayed.push((host.clone(), question.clone()));
        }
        if !relayed.is_empty() {
            core.harness.questions.changed(&mut state);
        }
    }
    for (host, question) in relayed {
        crate::server::hosts::route::relay_question(
            core,
            &host,
            serde_json::to_value(&question).unwrap_or_default(),
        );
    }
}

/// A question an agent on a linked host asked, which waits on the person
/// here, where its team is led from.
pub(crate) fn relayed_here(core: &Core, host: &str, params: Value) -> Result<Value, String> {
    let mut question: Question =
        serde_json::from_value(params).map_err(|error| error.to_string())?;
    let foreign = core
        .hosts
        .team
        .get(&question.agent_id)
        .filter(|foreign| foreign.host == host)
        .ok_or("that agent is not one of a team this host shares with yours")?;
    if question.text.len() > MAX_QUESTION_BYTES || question.choices.len() > MAX_CHOICES {
        return Err("that question is larger than this host takes".into());
    }
    question.name = format!(
        "{} on {}",
        foreign.name,
        crate::server::hosts::name_of(core, host)
    );
    question.project = foreign.project;
    question.relayed = None;
    question.relay_taken = false;
    question.undelivered = None;
    question.from_host = Some(host.to_owned());
    question.stage = match question.stage {
        Stage::Person { lead, passed } => Stage::Person { lead, passed },
        Stage::Lead { .. } => Stage::Person {
            lead: None,
            passed: false,
        },
    };
    let mut state = core.harness.questions.state.locked();
    state.open.retain(|open| open.id != question.id);
    state.open.push(question);
    core.harness.questions.changed(&mut state);
    Ok(Value::Null)
}

/// The host a question was relayed to took it, and lists it for phones from now on.
pub(crate) fn relay_arrived(core: &Core, id: &str) {
    let mut state = core.harness.questions.state.locked();
    let Some(question) = (state.open.iter_mut()).find(|question| question.id == id) else {
        return;
    };
    question.relay_taken = true;
    core.harness.questions.changed(&mut state);
}

/// The answer the person gave here to `asked`, a question an agent on `host` asked, was turned
/// away there for good: the question waits here again, saying why.
pub(crate) fn answer_turned_away(core: &Core, host: &str, asked: Value, why: &str) {
    let Ok(mut question) = serde_json::from_value::<Question>(asked) else {
        return;
    };
    question.undelivered = Some(format!(
        "Your answer did not reach {} on {}: {why}. Answer again, or dismiss it.",
        question.name.split(" on ").next().unwrap_or(&question.name),
        crate::server::hosts::name_of(core, host)
    ));
    let mut state = core.harness.questions.state.locked();
    state.open.retain(|open| open.id != question.id);
    state.open.push(question);
    core.harness.questions.changed(&mut state);
}

/// The person on the linked host this question was put to answered it.
pub(crate) async fn answer_relayed(
    core: &Arc<Core>,
    host: &str,
    params: Value,
) -> Result<Value, String> {
    let params: AnswerParams = serde_json::from_value(params).map_err(|error| error.to_string())?;
    let relayed_there = core
        .harness
        .questions
        .state
        .locked()
        .open
        .iter()
        .any(|question| question.id == params.id && question.relayed.as_deref() == Some(host));
    if !relayed_there {
        return Ok(json!({ "answered": false }));
    }
    answer(core, params).await?;
    Ok(json!({ "answered": true }))
}

/// A question an agent on a linked host asked no longer waits on the person.
pub(crate) fn closed_elsewhere(core: &Core, host: &str, id: &str) -> Result<Value, String> {
    let mut state = core.harness.questions.state.locked();
    let before = state.open.len();
    state
        .open
        .retain(|question| !(question.id == id && question.from_host.as_deref() == Some(host)));
    if state.open.len() != before {
        core.harness.questions.changed(&mut state);
    }
    Ok(Value::Null)
}
