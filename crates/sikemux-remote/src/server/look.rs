//! The host's half of a browser reading its look; [`sikemux_wire::look`] describes the exchange.

use crate::server::remote_state::RemoteParts;
use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::Connection;
use sikemux_client::join;
use sikemux_view::workspace::LookReply;
use sikemux_wire::accounts::protocol::JoinTicket;
use sikemux_wire::protocol::encode_control;

use super::Core;

const TICKET: Duration = Duration::from_secs(5);
const LINGER: Duration = Duration::from_secs(5);

pub async fn serve(core: Arc<Core>, connection: Connection) {
    let browser = connection.remote_id().to_string();
    let Ok(Ok((mut writer, mut reader))) =
        tokio::time::timeout(TICKET, connection.accept_bi()).await
    else {
        return;
    };
    let reply = match join::receive::<JoinTicket>(&mut reader, TICKET).await {
        Err(_) => LookReply::Refused {
            reason: "unreadable".into(),
        },
        Ok(ticket) => match core.remote().check_look(&ticket, &browser) {
            Err(refusal) => LookReply::Refused {
                reason: refusal.reason().into(),
            },
            Ok(()) => (core.remote_above().host_look)(&core),
        },
    };
    let Ok(frame) = encode_control(&reply) else {
        return;
    };
    let _ = writer.write_all(&frame).await;
    let _ = writer.finish();
    let _ = tokio::time::timeout(LINGER, connection.closed()).await;
}
