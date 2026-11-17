//! Answers the requests in [`crate::frozen`]: replacing this core
//! with a newer binary in the same process, restarting it from its own, and
//! stopping everything.

use crate::server::chat_state::ChatParts;
use crate::server::ServerCore;
use std::sync::Arc;
use std::time::Duration;

use sikemux_view::updates::UpdateBlocker;
use tokio::io::AsyncWriteExt;

use crate::frozen::{FrozenReply, FrozenRequest};
use sikemux_wire::protocol::encode_frozen;

use super::access::{Peer, APP_ONLY};
use super::connection::FrameWriter;
pub(crate) use super::updating::{Updating, UPDATING};
use super::Core;

#[cfg(unix)]
#[path = "upgrade/unix.rs"]
mod replace;
#[cfg(windows)]
#[path = "upgrade/windows.rs"]
mod replace;

pub(crate) use replace::replace_with;

/// How long a forced update waits for chats still starting, or in the middle
/// of a request, to be able to hand their agents over. One that still cannot
/// after that is stopped and starts again on its session.
pub(crate) const HANDOVER_SETTLE: Duration = Duration::from_secs(10);
/// How long an update nobody forced waits for that. With dozens of agents at work some
/// chat is nearly always busy, so after this it goes on as a forced one does.
pub(crate) const UPDATE_PATIENCE: Duration = Duration::from_secs(120);

pub(super) async fn reply(writer: &mut FrameWriter, answer: &FrozenReply) {
    if let Ok(frame) = encode_frozen(answer) {
        let _ = writer.write_all(&frame).await;
        let _ = writer.flush().await;
    }
}

pub(super) fn refused(message: impl Into<String>) -> FrozenReply {
    FrozenReply::Refused {
        message: message.into(),
    }
}

/// Answers one frozen request on a connection that sent it instead of a hello.
/// Only the app replaces the core: the binary it names takes over everything.
/// Any program here may restart it, which runs only the binary already running.
pub(crate) async fn answer(
    core: &Arc<Core>,
    peer: &Peer,
    payload: &[u8],
    writer: &mut FrameWriter,
) {
    match serde_json::from_slice::<FrozenRequest>(payload) {
        Ok(FrozenRequest::Upgrade { .. }) if *peer != Peer::App => {
            reply(writer, &refused(APP_ONLY)).await;
        }
        Ok(FrozenRequest::Upgrade { binary, force }) => {
            replace::upgrade(core, binary, force, writer).await
        }
        Ok(FrozenRequest::Restart) => replace::restart(core, writer).await,
        Ok(FrozenRequest::UpdateBlockers) => {
            let chats = core
                .chats()
                .blockers()
                .into_iter()
                .map(|blocker| UpdateBlocker {
                    agent_id: blocker.agent_id,
                    provider: blocker.provider,
                    title: blocker.title,
                    reason: blocker.reason,
                })
                .collect();
            reply(writer, &FrozenReply::UpdateBlockers { chats }).await;
        }
        Ok(FrozenRequest::StopEverything) => {
            reply(writer, &FrozenReply::Accepted).await;
            core.stop_everything().await;
            core.begin_shutdown();
        }
        Err(error) => reply(writer, &refused(format!("unknown request: {error}"))).await,
    }
}
