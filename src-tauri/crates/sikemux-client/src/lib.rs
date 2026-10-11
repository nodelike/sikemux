//! Talking to a core: from the app on the same machine, from a device that
//! paired with it, and a phone asking to join a host. The phone app links this
//! crate, so a change here is a change to the phone's native code and needs a
//! store build to reach it.

pub mod accounts;
pub mod client;
pub mod join;
pub mod remote;
