//! What a linked host may ask this one. Each ask is answered only while the
//! host that makes it is still linked.

use std::sync::Arc;

use serde_json::Value;

use super::super::Core;
use super::link::{Answer, Received};
use super::{answered, code, introduction, not_linked, open, route, spawn, transfer};

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
        "spawn" => spawn::start_here(core, host, params, body).await,
        "openProject" => open::open_here(core, params, body).await,
        "takeSession" => transfer::take_here(core, params, body).await,
        "hire" => answered(route::hire_here(core, host, params)),
        "forget" => answered(route::forget_here(core, host, params)),
        "deliver" => route::deliver_here(core, host, params).await,
        "read" => route::read_here(core, host, params).await,
        "happened" => answered(route::happened_here(core, host, params)),
        "team" => answered(route::team_here(core, host, params)),
        "worked" => answered(route::worked_here(core, host, params)),
        "commits" => code::land(core, host, params, body).await,
        "question" => super::super::questions::relayed_here(core, host, params),
        "answer" => super::super::questions::answer_relayed(core, host, params).await,
        "questionClosed" => super::super::questions::closed_elsewhere(core, host, params),
        "passOn" => route::pass_on_here(core, host, params),
        _ => Err(format!("this host does not know {method}")),
    }
}
