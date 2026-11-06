//! What a linked host may ask this one. Each ask is answered only while the
//! host that makes it is still linked.

use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use super::super::Core;
use super::link::{Answer, Received};
use super::{answered, code, introduction, not_linked, open, route, spawn, transfer};

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
    Hire "hire" route::HireParams;
    Forget "forget" route::AboutAgent;
    Read "read" route::ReadParams;
    Happened "happened" route::HappenedParams;
    Team "team" route::TeamParams;
    Worked "worked" route::AboutAgent;
    Commits "commits" code::ArrivingCommits;
    PassOn "passOn" route::PassOnParams;
    QuestionClosed "questionClosed" route::ClosedParams;
}

/// `params` as the ask `A` carries them.
pub(super) fn params_of<A: HostAsk>(params: &A::Params) -> Value {
    serde_json::to_value(params).unwrap_or_default()
}

fn read<A: HostAsk>(params: Value) -> Result<A::Params, String> {
    serde_json::from_value(params).map_err(|error| format!("unreadable {}: {error}", A::METHOD))
}

pub(super) async fn answer(
    core: &Arc<Core>,
    host: &str,
    method: &str,
    params: Value,
    body: Option<Received>,
) -> Answer {
    if core.remote.link(host).is_none() {
        return Err(not_linked(core, host).to_string());
    }
    match method {
        "join" => {
            let mut reply = introduction(core);
            reply["result"] = "allowed".into();
            Ok(reply)
        }
        "unlink" => {
            if core.remote.forget_link(host) {
                super::unlinked(core, host, "the person there removed the link");
            }
            Ok(Value::Null)
        }
        Spawn::METHOD => spawn::start_here(core, host, read::<Spawn>(params)?, body).await,
        OpenProject::METHOD => open::open_here(core, read::<OpenProject>(params)?, body).await,
        TakeSession::METHOD => transfer::take_here(core, read::<TakeSession>(params)?, body).await,
        Hire::METHOD => answered(route::hire_here(core, host, read::<Hire>(params)?)),
        Forget::METHOD => answered(route::forget_here(core, host, read::<Forget>(params)?)),
        "deliver" => route::deliver_here(core, host, params).await,
        Read::METHOD => route::read_here(core, host, read::<Read>(params)?).await,
        Happened::METHOD => answered(route::happened_here(core, host, read::<Happened>(params)?)),
        Team::METHOD => answered(route::team_here(core, host, read::<Team>(params)?)),
        Worked::METHOD => answered(route::worked_here(core, host, read::<Worked>(params)?)),
        Commits::METHOD => code::land(core, host, read::<Commits>(params)?, body).await,
        "question" => super::super::questions::relayed_here(core, host, params),
        "answer" => super::super::questions::answer_relayed(core, host, params).await,
        QuestionClosed::METHOD => {
            let params = read::<QuestionClosed>(params)?;
            super::super::questions::closed_elsewhere(core, host, &params.id)
        }
        PassOn::METHOD => route::pass_on_here(core, host, read::<PassOn>(params)?),
        _ => Err(format!("this host does not know {method}")),
    }
}
