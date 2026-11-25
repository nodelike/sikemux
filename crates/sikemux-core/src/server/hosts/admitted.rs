//! The devices and hosts each linked host let in, told to the others so each keeps a copy that
//! whoever came in cannot reach.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::server::remote_state::RemoteParts;

use super::super::remote::{self, Admission};
use super::super::Core;
use super::calls::{self as asks, params_of, HostAsk};
use super::link::Answer;
use super::{answered, Sending};

const LANE: &str = "admitted";

#[derive(Serialize, Deserialize)]
pub(super) struct Removed {
    key: String,
}

/// Tells every linked host who this one let in.
pub(crate) fn post(core: &Core, admission: &Admission) {
    let params = params_of::<asks::Admitted>(admission);
    to_each_link(core, &admission.key, asks::Admitted::METHOD, &params);
}

/// Has every linked host take the device `key` off.
pub(crate) fn post_removed(core: &Core, key: &str) {
    let params = params_of::<asks::RemoveDevice>(&Removed {
        key: key.to_owned(),
    });
    to_each_link(core, key, asks::RemoveDevice::METHOD, &params);
}

fn to_each_link(core: &Core, about: &str, method: &'static str, params: &Value) {
    for link in core.remote().links() {
        if link.id != about {
            let sending = Sending::new(method, params.clone()).kept().about(LANE);
            super::post(core, &link.id, sending);
        }
    }
}

pub(super) fn heard(core: &Core, host: &str, admission: Admission) -> Answer {
    let name = (core.remote().link(host)).map_or_else(|| host.to_owned(), |link| link.name);
    answered(remote::heard_admitted(core, name, admission))
}

pub(super) fn heard_removed(core: &Core, removed: &Removed) -> Answer {
    answered(remote::revoke(core, &removed.key).map(drop))
}
