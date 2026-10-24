//! What the core and its clients send each other, and nothing either side
//! does with it. The phone app links this crate, so a change here is a change
//! to the phone's native code and needs a store build to reach it.

pub mod accounts;
pub mod aws;
pub mod bruno;
pub mod chat;
pub mod codehost;
pub mod database;
pub mod grid;
pub mod jira;
pub mod linear;
pub mod notch;
pub mod protocol;
pub mod pty;
pub mod rundeck;
pub mod signoz;
pub mod slack;
pub mod theme;
pub mod vercel;
