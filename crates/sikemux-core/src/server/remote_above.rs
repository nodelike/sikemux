//! What pairing, the account, the vault, sharing and notifying ask of the features built over
//! them. They cannot name those features, so a core is built with these, one function for each.

use std::sync::Arc;

use sikemux_view::calls::PersonCall;

use super::Core;

pub(crate) struct RemoteAbove {
    /// Makes sure the connection runs while this host is signed in. The server closes it at once
    /// when the host has no webhooks.
    pub hooks_connect: fn(&Arc<Core>),
    /// Remote access went off: every link connection closes with it.
    pub hosts_halt: fn(&Core),
    /// The person here removed the link with `host`: the other host hears of it
    /// if it can, and the agents working across the link are let go.
    pub hosts_unlink: fn(&Core, &str),
    /// `host` is no longer linked with this one, for the reason `why`: what
    /// waits for it is dropped, and the leads of workers there are told.
    pub hosts_unlinked: fn(&Core, &str, &str),
    /// The open call the question `id` raised or joined.
    pub person_calls_asked_by: fn(&Core, &str) -> Option<PersonCall>,
    /// The call `id` while it waits on the person.
    pub person_calls_open_call: fn(&Core, &str) -> Option<PersonCall>,
    /// The open calls only this module tells the person of: those not raised by a question or a
    /// held action, which are told of as they always were.
    pub person_calls_raised_open: fn(&Core) -> Vec<PersonCall>,
    /// Whether the agent ends its turn on a wake it set itself: CI, a one-time timer, or a worker
    /// of its at work.
    pub turn_state_set_a_wake: fn(&Core, &str) -> bool,
}
