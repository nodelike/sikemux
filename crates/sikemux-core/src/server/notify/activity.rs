//! The Live Activity a phone shows of this host's agents, kept current while the phone is locked: the host
//! seals what its agents are doing for each phone that started one, and the account passes it on unread.

use crate::server::device_view::DeviceViewCore;
use crate::server::parts::Parts;
use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;
use sikemux_view::push::ActivityGlance;

use crate::accounts::live::Push;
use crate::accounts::protocol::PushKind;
use crate::push::{self, MAX_PLAINTEXT};
use crate::text::clipped_line;

use super::access::Peer;
use super::{Core, MAX_TITLE_CHARS};
use crate::server::CoreResult;

/// The push token of the Live Activity the calling phone shows this host's agents in; none once it ended.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ActivityParams {
    pub token: Option<String>,
}

/// Keeps the calling phone's Live Activity current from now on, or stops once it ended.
pub(crate) fn live_activity(core: &Core, peer: &Peer, params: ActivityParams) -> CoreResult<()> {
    let id = peer
        .device_id()
        .ok_or("only a phone shows a Live Activity")?;
    core.remote().set_activity(&id, params.token)?;
    core.notify().state.locked().activities.remove(&id);
    core.notify().inbox_moved();
    Ok(())
}

/// What a phone was last sent, so an unchanged glance is not sent again.
#[derive(Clone)]
pub(super) struct Sent {
    glance: ActivityGlance,
    token: String,
    at: u64,
}

/// A glance not followed by another this long goes stale on the phone.
const LIFE_MS: u64 = 15 * 60_000;
/// While anything works, an unchanged glance is sent again this often so it does not go stale.
const REFRESH_MS: u64 = 10 * 60_000;
/// What the activity's pushes are named by, so a newer one waiting replaces an older one.
const CARD: &str = "activity";

/// Sends each phone showing a Live Activity of this host what its agents do now, when that changed.
pub(super) fn tell(core: &Core, now: u64) {
    let Some((host, targets)) = core.remote().activity_targets() else {
        return;
    };
    if targets.is_empty() {
        core.notify().state.locked().activities.clear();
        return;
    }
    let (chats, sessions) = core.ranked_infos();
    let glance = crate::server::inbox::view(core, &chats, &sessions).glance;
    let outbox = core.remote().outbox();
    for (target, token) in targets {
        let last = core
            .notify()
            .state
            .locked()
            .activities
            .get(&target.device_id)
            .cloned();
        let fresh = last.as_ref().is_some_and(|last| {
            last.token == token
                && last.glance == glance
                && (!glance.active() || now.saturating_sub(last.at) < REFRESH_MS)
        });
        let ended = !glance.active() && last.as_ref().is_none_or(|last| !last.glance.active());
        if fresh || ended {
            continue;
        }
        let Some(blob) = sealed(&glance, &host, &target) else {
            continue;
        };
        let kind = if glance.active() {
            PushKind::Activity
        } else {
            PushKind::ActivityEnd
        };
        outbox.post(Push {
            to: target.device_id.clone(),
            kind,
            collapse_id: target.key.collapse_id(CARD),
            activity_token: Some(token.clone()),
            blob,
            expires_at: now + LIFE_MS,
        });
        let mut state = core.notify().state.locked();
        if kind == PushKind::ActivityEnd {
            state.activities.remove(&target.device_id);
            drop(state);
            core.remote().activity_gone(&target.device_id, &token);
        } else {
            let sent = Sent {
                glance: glance.clone(),
                token,
                at: now,
            };
            state.activities.insert(target.device_id.clone(), sent);
        }
    }
}

/// Whether a glance a phone holds is old enough to send again, so the tick builds one only then.
pub(super) fn refresh_due(core: &Core, now: u64) -> bool {
    (core.notify().state.locked().activities.values())
        .any(|sent| sent.glance.active() && now.saturating_sub(sent.at) >= REFRESH_MS)
}

fn sealed(glance: &ActivityGlance, host: &str, target: &super::NotifyTarget) -> Option<String> {
    let mut glance = glance.clone();
    for row in &mut glance.rows {
        row.title = clipped_line(&row.title, MAX_TITLE_CHARS);
    }
    for project in &mut glance.projects {
        project.name = clipped_line(&project.name, MAX_TITLE_CHARS);
    }
    let mut plaintext = serde_json::to_vec(&glance).ok()?;
    while plaintext.len() > MAX_PLAINTEXT && glance.rows.pop().is_some() {
        glance.more += 1;
        plaintext = serde_json::to_vec(&glance).ok()?;
    }
    push::seal(&target.key, host, &target.device_id, &plaintext)
}
