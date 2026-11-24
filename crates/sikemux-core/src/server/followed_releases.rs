//! Which releases this host follows, as its account hears it. The account decides by it whom a
//! feature in preview on nightly is open to.

use reqwest::Method;
use sikemux_view::prefs::UpdateChannel;

use super::account::api::host_request;
use super::remote_state::RemoteParts;
use super::updates::channel;
use super::{Core, CoreResult};
use crate::accounts::protocol::{FollowedChannel, FollowedReleases};

/// Tells the account. If it cannot be reached it is told when its connection is next up.
pub(super) fn tell(core: &Core) {
    let (Some(_), Some(core), Ok(runtime)) = (
        core.remote().owner(),
        core.arc(),
        tokio::runtime::Handle::try_current(),
    ) else {
        return;
    };
    runtime.spawn(async move {
        if let Err(error) = put(&core).await {
            eprintln!("sikemux core: the account was not told which releases this computer follows: {error}");
        }
    });
}

async fn put(core: &Core) -> CoreResult<()> {
    let channel = match channel(core) {
        UpdateChannel::Stable => FollowedChannel::Stable,
        UpdateChannel::Nightly => FollowedChannel::Nightly,
        UpdateChannel::Staff => FollowedChannel::Staff,
    };
    let body = serde_json::to_value(FollowedReleases { channel })?;
    host_request(
        core,
        Method::PUT,
        "/v1/account/followed-releases",
        Some(body),
    )
    .await?;
    Ok(())
}
