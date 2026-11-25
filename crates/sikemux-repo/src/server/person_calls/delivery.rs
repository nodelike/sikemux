use std::sync::Arc;
use std::time::Duration;

use sikemux_calls::questions::AnswerParams;
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use sikemux_view::calls::{
    AnswerCallParams, CallAnswer, CallClient, CallKind, CallResult, PersonCall, DO_NOT_DOING,
};

use super::log::Entry;
use super::{
    attached, client_of, kept, key_of, open, outcome, AfterStart, Source, ACCOUNT_REFUSED, GONE,
    NOT_A_CLIENT, ORG_WAITING,
};
use crate::server::access::Peer;
use crate::server::edits::workflows;
use crate::server::remote_state::RemoteParts;
use crate::server::repo_state::RepoParts;
use crate::server::{ship, Core, CoreResult};

/// Whether a production place may open on this answer. The gates by place decide it.
pub(super) fn may_pass(
    core: &Core,
    call: &PersonCall,
    peer: &Peer,
    picked: bool,
) -> Result<(), String> {
    crate::server::places::may_pass(core, call, peer, picked)
}

/// A yes to a production call as the person reads it before it counts: who asks, for what,
/// and what they picked. None for a no.
pub(super) fn yes_in_words(call: &PersonCall, answer: &CallAnswer) -> Option<String> {
    let picked: Vec<&str> = (answer.option.iter().chain(&answer.options))
        .filter(|id| *id != HOLD_REFUSE)
        .filter_map(|id| call.options.iter().find(|option| option.id == *id))
        .map(|option| option.label.as_str())
        .collect();
    if picked.is_empty() {
        return None;
    }
    let mut words = format!("@{} asks: {}", call.asker.name, call.question);
    if let Some(action) = &call.action {
        words.push_str(&format!("\n\n{action}"));
    }
    words.push_str(&format!("\n\nYour answer: {}", picked.join(", ")));
    Some(words)
}

/// What the person's answer is, as the call keeps it, and what they typed with it. A secret is
/// never kept.
fn answer_of(
    call: &PersonCall,
    client: CallClient,
    params: &AnswerCallParams,
) -> CoreResult<(CallAnswer, Option<String>)> {
    let text = |text: &Option<String>| {
        text.as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let (note, words) = (text(&params.note), text(&params.words));
    let known = |id: &String| call.options.iter().any(|option| option.id == *id);
    if params.option.as_ref().is_some_and(|id| !known(id)) {
        return Err("that is not one of the call's options".into());
    }
    let picked = params.option.is_some();
    if picked && words.is_some() {
        return Err("answer with an option and a note, or in your own words".into());
    }
    if !picked && words.is_none() {
        return Err("pick an option or answer in your own words".into());
    }
    if params.option.as_deref() == Some(DO_NOT_DOING) && note.is_none() {
        return Err("say why you're not doing it".into());
    }
    let typed = note.clone().or(words.clone());
    let refused = call.kind == CallKind::Production
        && !picked
        && call.options.iter().any(|option| option.id == HOLD_REFUSE);
    let (option, note, words) = match refused {
        true => (Some(HOLD_REFUSE.to_owned()), words, None),
        false => (params.option.clone(), note, words),
    };
    let answer = CallAnswer {
        option,
        options: Vec::new(),
        note: note.filter(|_| !call.secret),
        words: words.filter(|_| !call.secret),
        none: false,
        by: client,
        at: epoch_ms(),
        waited_ms: epoch_ms().saturating_sub(call.raised_at),
    };
    Ok((answer, typed))
}

pub(super) fn call_of(core: &Core, id: &str) -> CoreResult<PersonCall> {
    Ok(open(&core.person_calls().book.locked(), id)?.call.clone())
}

/// The person answers a call from the app, the notch or a phone. A production call is answered
/// only on a client that drew it, and only an option picked there can pass its place: words say
/// no, or what to do instead. The reply comes once the answer is on disk and has reached the
/// asker, or has gone on trying for a while; the call opens again if it cannot reach it.
pub async fn answer(core: &Arc<Core>, peer: &Peer, params: AnswerCallParams) -> CoreResult<()> {
    let client = client_of(peer).ok_or(NOT_A_CLIENT)?;
    let call = call_of(core, &params.id)?;
    let (answer, typed) = answer_of(&call, client.clone(), &params)?;
    if call.secret && !params.paths.is_empty() {
        return Err("a secret is typed into the call, never attached".into());
    }
    crate::server::chat::prompt::check_attachments(&params.paths)?;
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
        may_pass(
            core,
            &call,
            peer,
            answer.option.is_some() || !answer.options.is_empty(),
        )?;
        if let Some(words) = yes_in_words(&call, &answer) {
            core.remote().sign(peer, &words).await?;
        }
    }
    let (call, source) = core.person_calls().take_answer(&call.id, answer)?;
    if let Err(error) = core.person_calls().starting_on_yes(&call.id) {
        return Err(core.person_calls().unanswer(core, &call.id, error));
    }
    core.person_calls().changed(core);
    let core = core.clone();
    let mut delivering = tokio::spawn(async move {
        let reached = Box::pin(deliver(&core, &call, &source, typed, params.paths)).await;
        if reached.is_err() {
            core.person_calls().reopen(&call.id);
            core.person_calls().changed(&core);
        }
        reached
    });
    match tokio::time::timeout(REPLY_WAITS_FOR_DELIVERY, &mut delivering).await {
        Ok(reached) => reached.unwrap_or(Ok(())),
        Err(_) => {
            tokio::spawn(async move {
                if let Ok(Err(error)) = delivering.await {
                    eprintln!(
                        "sikemux core: an answered call did not reach its asker, so it is open again: {error}"
                    );
                }
            });
            Ok(())
        }
    }
}

/// How long answering a call waits for the answer to reach the asker, so a quick failure still
/// says why in the reply.
const REPLY_WAITS_FOR_DELIVERY: Duration = Duration::from_secs(2);

/// Takes the answer to whatever raised the call.
pub(super) async fn deliver(
    core: &Arc<Core>,
    call: &PersonCall,
    source: &Source,
    typed: Option<String>,
    paths: Vec<String>,
) -> CoreResult<()> {
    let answer = call.answer.as_ref().ok_or(GONE)?;
    let listed = || match (&typed, paths.is_empty()) {
        (_, true) => typed.clone(),
        (Some(typed), false) => Some(format!("{typed}\n\n{}", attached(&paths))),
        (None, false) => Some(attached(&paths)),
    };
    match source {
        Source::Question(_) => {
            let index = |option: &String| call.options.iter().position(|known| known.id == *option);
            let ids = core.person_calls().questions_of(&call.id);
            if ids.is_empty() {
                return Err(GONE.into());
            }
            let mut failed = None;
            for id in ids {
                let params = AnswerParams {
                    id,
                    answer: typed.clone(),
                    paths: paths.clone(),
                    choice: answer.option.as_ref().and_then(index),
                    ..AnswerParams::default()
                };
                if let Err(error) = (core.repo_above().questions_answer)(core, params).await {
                    failed = failed.or(Some(error));
                }
            }
            if let Some(error) = failed {
                return Err(error.into());
            }
            core.person_calls().settle(&call.id, CallResult::Heard);
        }
        Source::Hold(hold) => {
            let params = ship::HoldParams { hold: hold.clone() };
            match answer.option.as_deref() {
                Some(HOLD_ALLOW) => ship::allow(core, params).await?,
                _ => {
                    ship::refuse(core, params).await?;
                    core.person_calls().settle(&call.id, CallResult::NotShipped);
                }
            }
            if let Some(typed) = listed() {
                let text = format!("The person added: {typed}");
                let _ = (core.repo_above().messages_for_person)(
                    core,
                    call.asker.agent_id.clone(),
                    text,
                )
                .await;
            }
        }
        Source::Raised if call.place.is_some() => call_answered(core, call, listed()).await,
        Source::Raised
            if key_of(core, &call.id).is_some_and(|key| key.starts_with(ACCOUNT_REFUSED)) =>
        {
            let key = key_of(core, &call.id).unwrap_or_default();
            (core.repo_above().accounts_call_answered)(core, call, &key).await;
        }
        Source::Raised
            if key_of(core, &call.id).is_some_and(|key| key.starts_with(ORG_WAITING)) =>
        {
            let key = key_of(core, &call.id).unwrap_or_default();
            (core.repo_above().memory_call_answered)(core, call, &key);
        }
        Source::Raised if key_of(core, &call.id).is_some_and(|key| workflows::is_key(&key)) => {
            let key = key_of(core, &call.id).unwrap_or_default();
            Box::pin(workflows::answered(core, call, &key, listed())).await;
        }
        Source::Raised => {
            tell_askers(core, call, listed()).await;
            core.person_calls().settle(&call.id, CallResult::Heard);
        }
    }
    Ok(())
}

/// A production call the gates by place raised was answered.
async fn call_answered(core: &Arc<Core>, call: &PersonCall, typed: Option<String>) {
    crate::server::places::call_answered(core, call);
    ship::gate_answered(core, call, typed).await;
}

/// Tells every agent waiting on the call what the person answered.
async fn tell_askers(core: &Arc<Core>, call: &PersonCall, typed: Option<String>) {
    let Some(said) = said(call, typed) else {
        return;
    };
    let text = format!("The person answered \"{}\":\n{said}", call.question);
    tell(core, call, &text).await;
}

/// What the person answered, in words: the option they picked and what they typed with it.
fn said(call: &PersonCall, typed: Option<String>) -> Option<String> {
    let answer = call.answer.as_ref()?;
    let picked = (answer.option.iter().chain(&answer.options))
        .filter_map(|id| call.options.iter().find(|option| option.id == *id))
        .map(|option| option.label.clone());
    let said: Vec<String> = picked.chain(typed).collect();
    (!said.is_empty()).then(|| said.join("\n\n"))
}

/// The option of a held action that lets it run once.
pub const HOLD_ALLOW: &str = "allow";
pub const HOLD_REFUSE: &str = "refuse";

/// Finishes what a restart cut short. The agents waiting on a yes hear whether what it let run
/// never started, or started and may have run; an agent whose action was held hears to run it
/// again; a gate's call whose yes would no longer count is taken back; and each other answer
/// reaches its askers if it had not. A stopped agent hears when it starts again.
pub(super) async fn after_restart(core: &Arc<Core>, left: AfterStart) {
    let lost = {
        let core = core.clone();
        let taken_back = move || {
            (left.gated.into_iter())
                .filter(|id| {
                    !crate::server::ship::yes_counts(&core, id) && take_back_lost(&core, id)
                })
                .collect::<Vec<String>>()
        };
        tokio::task::spawn_blocking(taken_back)
            .await
            .unwrap_or_default()
    };
    let said = [
        (
            left.lapsed,
            "Sikemux restarted before your action ran, so the person's yes to \"{}\" lapsed. Nothing ran. Try it again when you need it; that asks them again.",
        ),
        (
            left.interrupted,
            "Sikemux restarted while your action ran after the person's yes to \"{}\", so it may have run. Check what happened before you try it again.",
        ),
        (
            left.dropped,
            "Sikemux restarted, so your action held for the person's answer to \"{}\" no longer waits. Run it again when you need it; that asks them again.",
        ),
        (
            lost,
            "Sikemux restarted, and a yes to \"{}\" would no longer count, so it took that call back. Run it again when you need it; that asks them again.",
        ),
    ];
    for (ids, words) in said {
        for id in ids {
            if let Some(call) = call_any(core, &id) {
                let text = words.replace("{}", &call.question);
                tell(core, &call, &text).await;
            }
        }
    }
    for id in left.undelivered {
        redeliver(core, &id).await;
    }
    core.person_calls().changed(core);
}

/// Closes the gate's call `id`, unless the person answered it meanwhile.
fn take_back_lost(core: &Core, id: &str) -> bool {
    let mut book = core.person_calls().book.locked();
    let waits = open(&book, id).is_ok();
    if waits {
        let came = Entry::Came {
            id: id.to_owned(),
            outcome: outcome(CallResult::Withdrawn),
        };
        core.person_calls().write(&mut book, came);
    }
    waits
}

fn call_any(core: &Core, id: &str) -> Option<PersonCall> {
    kept(&core.person_calls().book.locked(), id).map(|kept| kept.call.clone())
}

/// Tells every agent waiting on the call, waiting for any that has stopped.
async fn tell(core: &Arc<Core>, call: &PersonCall, text: &str) {
    for asker in std::iter::once(&call.asker).chain(&call.joined) {
        let to = asker.agent_id.clone();
        let _ = (core.repo_above().messages_for_person_waiting)(core, to, text.to_owned()).await;
    }
}

/// Gives an answer a restart may have kept from its askers. An answer whose question is no longer
/// open may have been lost between being taken and being sent, so it is sent as a plain message;
/// one whose words were never kept, as an access call's, waits on the person again.
async fn redeliver(core: &Arc<Core>, id: &str) {
    let Some((call, source)) = kept(&core.person_calls().book.locked(), id)
        .map(|kept| (kept.call.clone(), kept.source.clone()))
    else {
        return;
    };
    let Some(answer) = &call.answer else {
        return;
    };
    let typed = answer.note.clone().or(answer.words.clone());
    let picked = answer.option.is_some() || !answer.options.is_empty();
    if let Source::Hold(_) = source {
        if let Some(said) = said(&call, typed) {
            let text = format!(
                "Sikemux restarted as the person answered \"{}\", so the action it held no longer waits. They said:\n{said}",
                call.question
            );
            tell(core, &call, &text).await;
        }
        core.person_calls().settle(id, CallResult::NotShipped);
        return;
    }
    if let Source::Question(_) = source {
        let questions = core.person_calls().questions_of(id);
        let open = (questions.iter())
            .any(|question| (core.repo_above().questions_is_open)(core, question));
        if !open {
            let text = match said(&call, typed.clone()) {
                Some(said) => format!(
                    "The person answered \"{}\" as Sikemux restarted, and the answer may not have reached you:\n{said}",
                    call.question
                ),
                None => format!(
                    "The person answered \"{}\" as Sikemux restarted, and the answer was not kept. Ask them again if you still need it.",
                    call.question
                ),
            };
            tell(core, &call, &text).await;
            core.person_calls().settle(id, CallResult::Heard);
            return;
        }
        if !picked && typed.is_none() {
            core.person_calls().reopen(id);
            return;
        }
    }
    if deliver(core, &call, &source, typed, Vec::new())
        .await
        .is_err()
    {
        core.person_calls().reopen(id);
    }
}
