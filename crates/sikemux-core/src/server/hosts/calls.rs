//! What a linked host may ask this one. Each ask is answered only while the
//! host that makes it is still linked.

use crate::server::parts::Parts;
use crate::server::remote_state::RemoteParts;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use super::super::Core;
use super::link::{Answer, Kind, Received, Refusal};
use super::receipts::Stands;
use super::{
    answered, chat_move, code, copies, introduction, not_linked, open, route, spawn, transfer,
};

/// One ask between hosts: what it is called and what it carries, shared by the host asking
/// and the host answering, which may run different builds.
pub(super) trait HostAsk {
    const METHOD: &'static str;
    type Params: Serialize + DeserializeOwned;
}

macro_rules! host_asks {
    ($($marker:ident $method:literal $params:ty;)*) => {
        $(
            pub(super) struct $marker;

            impl HostAsk for $marker {
                const METHOD: &'static str = $method;
                type Params = $params;
            }
        )*
    };
}

host_asks! {
    Spawn "spawn" spawn::SpawnParams;
    OpenProject "openProject" open::OpenParams;
    TakeSession "takeSession" transfer::Take;
    UntakeSession "untakeSession" transfer::Untake;
    Hire "hire" route::HireParams;
    Forget "forget" route::AboutAgent;
    Read "read" route::ReadParams;
    Close "close" route::CloseParams;
    Permit "permit" route::PermitParams;
    Act "act" route::ActParams;
    Happened "happened" route::HappenedParams;
    Team "team" route::TeamParams;
    Worked "worked" route::AboutAgent;
    Commits "commits" code::ArrivingCommits;
    CommitsReady "commitsReady" code::ArrivingCommits;
    PullCommits "pullCommits" code::ArrivingCommits;
    AutoTurn "autoTurn" spawn::AutoTurn;
    Stop "stop" route::AboutAgent;
    PassOn "passOn" route::PassOnParams;
    QuestionClosed "questionClosed" route::ClosedParams;
    Copies "copies" copies::HeldParams;
    TakeChat "takeChat" chat_move::TakeChat;
    Receipt "receipt" ReceiptParams;
    Workers "workers" route::WorkersParams;
}

/// Asks what became of the call `call`.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct ReceiptParams {
    pub(super) call: String,
}

pub(super) fn params_of<A: HostAsk>(params: &A::Params) -> Value {
    serde_json::to_value(params).unwrap_or_default()
}

fn read<A: HostAsk>(params: Value) -> Result<A::Params, String> {
    serde_json::from_value(params).map_err(|error| format!("unreadable {}: {error}", A::METHOD))
}

/// The file a linked host fetches for a post this host sent it.
pub(super) fn pulled(
    core: &Core,
    host: &str,
    method: &str,
    params: Value,
) -> Result<std::path::PathBuf, Refusal> {
    if core.remote().link(host).is_none() {
        return Err(Refusal::not_linked(not_linked(core, host).to_string()));
    }
    match method {
        PullCommits::METHOD => spawn::commits_for(core, host, read::<PullCommits>(params)?),
        _ => Err(format!("this host does not know {method}").into()),
    }
}

pub(super) async fn answer(
    core: &Arc<Core>,
    host: &str,
    method: &str,
    params: Value,
    body: Option<Received>,
    call: Option<String>,
) -> Result<Value, Refusal> {
    if core.remote().link(host).is_none() {
        return Err(Refusal::not_linked(not_linked(core, host).to_string()));
    }
    if method == Receipt::METHOD {
        let params = read::<Receipt>(params)?;
        return Ok(core.hosts().receipts.stands(host, &params.call).told());
    }
    let Some(call) = call else {
        return work(core, host, method, params, body).await;
    };
    let doing = match core.hosts().receipts.begin(host, &call) {
        Ok(doing) => doing,
        Err(Stands::Done(answer)) => return answer.map_err(Refusal::from),
        Err(_) => return Err(Refusal::retry("this host is still doing that")),
    };
    let answer = work(core, host, method, params, body).await;
    match &answer {
        Ok(value) if value.get("needs").is_none() => doing.done(&Ok(value.clone())),
        Err(refusal) if refusal.kind == Kind::Final => doing.done(&Err(refusal.why.clone())),
        _ => {}
    }
    answer
}

async fn work(
    core: &Arc<Core>,
    host: &str,
    method: &str,
    params: Value,
    body: Option<Received>,
) -> Result<Value, Refusal> {
    if method == Commits::METHOD || method == CommitsReady::METHOD {
        return code::land(core, host, read::<Commits>(params)?, body).await;
    }
    dispatch(core, host, method, params, body)
        .await
        .map_err(Refusal::from)
}

async fn dispatch(
    core: &Arc<Core>,
    host: &str,
    method: &str,
    params: Value,
    body: Option<Received>,
) -> Answer {
    match method {
        "join" => {
            let mut reply = introduction(core);
            reply["result"] = "allowed".into();
            Ok(reply)
        }
        "unlink" => {
            if core.remote().forget_link(host) {
                super::unlinked(core, host, "the person there removed the link");
            }
            Ok(Value::Null)
        }
        Spawn::METHOD => spawn::start_here(core, host, read::<Spawn>(params)?, body).await,
        OpenProject::METHOD => open::open_here(core, read::<OpenProject>(params)?, body).await,
        TakeSession::METHOD => transfer::take_here(core, read::<TakeSession>(params)?, body).await,
        UntakeSession::METHOD => transfer::untake_here(core, read::<UntakeSession>(params)?).await,
        Hire::METHOD => answered(route::hire_here(core, host, read::<Hire>(params)?)),
        Forget::METHOD => answered(route::forget_here(core, host, read::<Forget>(params)?)),
        "deliver" => route::deliver_here(core, host, params).await,
        Read::METHOD => route::read_here(core, host, read::<Read>(params)?).await,
        Close::METHOD => answered(route::close_here(core, host, read::<Close>(params)?)),
        Permit::METHOD => answered(route::permit_here(core, host, read::<Permit>(params)?)),
        Act::METHOD => answered(route::act_here(core, host, read::<Act>(params)?).await),
        Workers::METHOD => route::workers_here(core, host, read::<Workers>(params)?),
        Happened::METHOD => answered(route::happened_here(core, host, read::<Happened>(params)?)),
        Team::METHOD => answered(route::team_here(core, host, read::<Team>(params)?)),
        Worked::METHOD => answered(route::worked_here(core, host, read::<Worked>(params)?)),
        AutoTurn::METHOD => {
            super::super::board::elsewhere::turn_ended(core, host, read::<AutoTurn>(params)?)
        }
        Stop::METHOD => answered(route::stop_here(core, host, read::<Stop>(params)?)),
        "question" => super::super::questions::relayed_here(core, host, params),
        "answer" => super::super::questions::answer_relayed(core, host, params).await,
        QuestionClosed::METHOD => {
            let params = read::<QuestionClosed>(params)?;
            super::super::questions::closed_elsewhere(core, host, &params.id)
        }
        PassOn::METHOD => route::pass_on_here(core, host, read::<PassOn>(params)?),
        Copies::METHOD => copies::heard(core, host, read::<Copies>(params)?),
        TakeChat::METHOD => chat_move::take_here(core, read::<TakeChat>(params)?).await,
        _ => Err(format!("this host does not know {method}")),
    }
}
