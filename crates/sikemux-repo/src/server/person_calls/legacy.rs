use std::sync::Arc;

use sikemux_calls::questions::{AnswerParams, AnswersParams};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::calls::{CallAnswer, CallClient, CallKind, CallParams, CallResult, PersonCall};

use super::delivery::{call_of, deliver, may_pass, yes_in_words, HOLD_ALLOW, HOLD_REFUSE};
use super::{client_of, gone, kept, raised_by, Source, NOT_A_CLIENT};
use crate::server::access::Peer;
use crate::server::remote_state::RemoteParts;
use crate::server::repo_state::RepoParts;
use crate::server::{ship, Core, CoreResult};

/// An answer from a released client that names the question, the hold or the action: it counts as
/// the call's, and drawing the call is taken as given, since only a client that drew it has its id.
fn answered_before(
    core: &Core,
    (peer, client): (&Peer, CallClient),
    source: &Source,
    answer: impl FnOnce(&PersonCall, CallClient) -> CallAnswer,
) -> CoreResult<Option<String>> {
    let Some(id) = core.person_calls().id_of(source) else {
        let settled =
            (core.person_calls().book.locked().iter()).any(|kept| raised_by(kept, source));
        return match settled {
            true => Err(gone()),
            false => Ok(None),
        };
    };
    let call = call_of(core, &id)?;
    if call.kind == CallKind::Money {
        core.remote().may_sign(peer)?;
    }
    let given = answer(&call, client);
    if call.kind == CallKind::Production {
        may_pass(
            core,
            &call,
            peer,
            given.option.is_some() || !given.options.is_empty(),
        )?;
    }
    core.person_calls().take_answer(&id, given)?;
    if let Err(error) = core.person_calls().starting_on_yes(&id) {
        return Err(core.person_calls().unanswer(core, &id, error));
    }
    core.person_calls().changed(core);
    Ok(Some(id))
}

const PRODUCTION_ONLY_ON_ITS_CALL: &str = "This asks for a change to production, which is answered only by a tap on its ask. Open the ask to answer it; on a phone that shows no asks, answer it on the Mac or update the app";

/// `answerQuestion` and `answerQuestions`, from clients released before calls: each answer is its
/// call's.
pub async fn answer_questions(
    core: &Arc<Core>,
    peer: &Peer,
    given: AnswersParams,
) -> CoreResult<()> {
    let client = client_of(peer).ok_or(NOT_A_CLIENT)?;
    answer_questions_by(core, (peer, client), given).await
}

async fn answer_questions_by(
    core: &Arc<Core>,
    by: (&Peer, CallClient),
    given: AnswersParams,
) -> CoreResult<()> {
    if (given.answers.iter())
        .any(|params| (core.repo_above().questions_asks_production)(core, &params.id))
    {
        return Err(PRODUCTION_ONLY_ON_ITS_CALL.into());
    }
    if !(given.answers.iter()).all(|params| (core.repo_above().questions_is_open)(core, &params.id))
    {
        return Err(gone());
    }
    let answered: Vec<&str> = given
        .answers
        .iter()
        .map(|params| params.id.as_str())
        .collect();
    let mut joined = Vec::new();
    for params in &given.answers {
        let Some(call) = core
            .person_calls()
            .id_of(&Source::Question(params.id.clone()))
        else {
            continue;
        };
        let waiting = core.person_calls().questions_of(&call);
        for id in waiting
            .into_iter()
            .filter(|id| !answered.contains(&id.as_str()))
        {
            joined.push(AnswerParams {
                id,
                answer: params.answer.clone(),
                paths: params.paths.clone(),
                choice: params.choice,
                ..AnswerParams::default()
            });
        }
    }
    let mut taken = Vec::new();
    for params in &given.answers {
        let taken_now = answered_before(
            core,
            by.clone(),
            &Source::Question(params.id.clone()),
            |call, by| {
                let option = |index: usize| call.options.get(index).map(|option| option.id.clone());
                let typed =
                    (params.answer.clone()).filter(|text| !text.trim().is_empty() && !call.secret);
                let picked = params.choice.is_some();
                CallAnswer {
                    option: params.choice.and_then(option),
                    options: Vec::new(),
                    note: typed.clone().filter(|_| picked),
                    words: typed.filter(|_| !picked),
                    none: false,
                    by,
                    at: epoch_ms(),
                    waited_ms: epoch_ms().saturating_sub(call.raised_at),
                }
            },
        );
        match taken_now {
            Ok(id) => taken.extend(id),
            Err(error) => {
                taken
                    .iter()
                    .for_each(|id: &String| core.person_calls().reopen(id));
                return Err(error);
            }
        }
    }
    match (core.repo_above().questions_answer_all)(core, given).await {
        Ok(()) => {
            for params in joined {
                let _ = (core.repo_above().questions_answer)(core, params).await;
            }
            for id in &taken {
                core.person_calls().settle(id, CallResult::Heard);
            }
            core.person_calls().changed(core);
            Ok(())
        }
        Err(error) => {
            taken.iter().for_each(|id| core.person_calls().reopen(id));
            core.person_calls().changed(core);
            Err(error.into())
        }
    }
}

fn raised_by_gate(core: &Core, id: &str) -> bool {
    (kept(&core.person_calls().book.locked(), id))
        .is_some_and(|kept| kept.source == Source::Raised && kept.call.place.is_some())
}

/// A released client answers a production call a gate raised by its hold's id.
async fn answer_gate(core: &Arc<Core>, peer: &Peer, id: &str, option: &str) -> CoreResult<()> {
    let client = client_of(peer).ok_or(NOT_A_CLIENT)?;
    let call = call_of(core, id)?;
    if !call.options.iter().any(|known| known.id == option) {
        return Err("that is not one of the call's options".into());
    }
    if matches!(call.kind, CallKind::Production | CallKind::Money) {
        core.remote().may_sign(peer)?;
    }
    if call.kind == CallKind::Production {
        if !core.person_calls().seen_by(&call.id, &client) {
            return Err(
                "open the call to answer it: a production call is answered where it was seen"
                    .into(),
            );
        }
        may_pass(core, &call, peer, true)?;
    }
    let given = CallAnswer {
        option: Some(option.to_owned()),
        options: Vec::new(),
        note: None,
        words: None,
        none: false,
        by: client,
        at: epoch_ms(),
        waited_ms: epoch_ms().saturating_sub(call.raised_at),
    };
    if call.kind == CallKind::Production {
        if let Some(words) = yes_in_words(&call, &given) {
            core.remote().sign(peer, &words).await?;
        }
    }
    let (call, source) = core.person_calls().take_answer(id, given)?;
    core.person_calls().changed(core);
    deliver(core, &call, &source, None, Vec::new()).await
}

fn hold_signing_words(core: &Core, source: &Source, hold_id: &str) -> String {
    let action_label = "Let this go out once";
    if let Some(id) = core.person_calls().id_of(source) {
        if let Ok(call) = call_of(core, &id) {
            let mut words = format!("@{} asks: {}", call.asker.name, call.question);
            if let Some(place) = &call.place {
                words.push_str(&format!("\nPlace: {}", place.name));
            }
            if let Some(action) = &call.action {
                words.push_str(&format!("\n\n{action}"));
            }
            words.push_str(&format!("\n\nYour answer: {action_label}"));
            return words;
        }
    }
    if let Some(waiting) = core
        .ship()
        .waiting()
        .into_iter()
        .find(|waiting| waiting.hold.id == hold_id)
    {
        let mut words = format!("{action_label}:\n\n{}", waiting.hold.what);
        if let Some(target) = waiting.writes_to {
            words.push_str(&format!("\nPlace: {target}"));
        }
        return words;
    }
    "Let an agent's held work go out once.".into()
}

/// `allowShip` and `refuseShip`, from clients released before calls.
pub async fn answer_hold(
    core: &Arc<Core>,
    peer: &Peer,
    params: ship::HoldParams,
    option: &str,
) -> CoreResult<()> {
    if raised_by_gate(core, &params.hold) {
        return answer_gate(core, peer, &params.hold, option).await;
    }
    let source = Source::Hold(params.hold.clone());
    let client = client_of(peer).ok_or(NOT_A_CLIENT)?;
    if let Some(id) = core.person_calls().id_of(&source) {
        let call = call_of(core, &id)?;
        if call.kind == CallKind::Production && !core.person_calls().seen_by(&call.id, &client) {
            return Err(
                "open the call to answer it: a production call is answered where it was seen"
                    .into(),
            );
        }
    }
    if option != HOLD_REFUSE {
        core.remote().may_sign(peer)?;
        let words = hold_signing_words(core, &source, &params.hold);
        core.remote().sign(peer, &words).await?;
    }
    let taken = answered_before(core, (peer, client), &source, |call, by| CallAnswer {
        option: Some(option.to_owned()),
        options: Vec::new(),
        note: None,
        words: None,
        none: false,
        by,
        at: epoch_ms(),
        waited_ms: epoch_ms().saturating_sub(call.raised_at),
    })?;
    let ran = match option {
        HOLD_ALLOW => ship::allow(core, params).await,
        _ => ship::refuse(core, params).await,
    };
    if let Some(id) = &taken {
        match &ran {
            Ok(()) if option == HOLD_REFUSE => {
                core.person_calls().settle(id, CallResult::NotShipped)
            }
            Ok(()) => {}
            Err(_) => core.person_calls().reopen(id),
        }
        core.person_calls().changed(core);
    }
    ran
}

/// What an earlier release's Not mine sends. Nothing is handed back now: the person answers.
pub async fn not_mine(_core: &Arc<Core>, _peer: &Peer, _params: CallParams) -> CoreResult<()> {
    Err("Nothing is handed back: answer it, or tell the agent in its chat".into())
}
