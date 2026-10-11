//! What the core and its clients send each other, and nothing either side
//! does with it. The phone app links this crate, so a change here is a change
//! to the phone's native code and needs a store build to reach it.

pub mod accounts;
pub mod cli;
pub mod protocol;
pub mod pty;
