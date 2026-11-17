//! What the signed-in account's plan allows, kept on disk so a host that cannot reach the
//! account still knows. The account says plan.changed when it changes, and the host reads it
//! again then and each time its live connection comes back.

use std::path::Path;

use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

use super::api::host_send_any;
use super::{folder, show};
use crate::accounts::protocol::{Allowance, PlanFeature, PlanId};
use crate::server::remote_state::RemoteParts;
use crate::server::Core;

const FILE: &str = "allowance.json";
const PATH: &str = "/v1/account/allowance";

#[derive(Serialize, Deserialize)]
struct Kept {
    user_id: String,
    allowance: Allowance,
}

/// What `user_id`'s plan allows, or nothing while the account has not said.
pub async fn read(dir: &Path, user_id: &str) -> Option<Allowance> {
    let text = tokio::fs::read(dir.join(FILE)).await.ok()?;
    let kept: Kept = serde_json::from_slice(&text).ok()?;
    (kept.user_id == user_id).then_some(kept.allowance)
}

/// Whether the account may use Pro's features without paying for them: before billing is live,
/// or while Pro is a gift.
pub fn pro_given(allowance: &Allowance) -> bool {
    let has_pro = allowance.features.iter().any(|feature| {
        matches!(
            feature,
            PlanFeature::Sync
                | PlanFeature::MoveWork
                | PlanFeature::HistorySearch
                | PlanFeature::OwnTriggers
        )
    });
    allowance.gift_ends.is_some() || (allowance.plan == PlanId::Free && has_pro)
}

/// Asks the account what its plan allows and shows the answer. An account server that does not
/// know the question yet leaves everything allowed; one that cannot be reached leaves the last answer.
pub async fn refresh(core: &Core) {
    let (Some(user_id), Some(dir)) = (core.remote().owner(), folder(core)) else {
        return;
    };
    let Ok(answer) = host_send_any(core, Method::GET, PATH, None).await else {
        return;
    };
    let path = dir.join(FILE);
    let written = match answer.status() {
        StatusCode::NOT_FOUND => match tokio::fs::remove_file(&path).await {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        },
        status if status.is_success() => {
            let Ok(allowance) = answer.json::<Allowance>().await else {
                return;
            };
            let Ok(bytes) = serde_json::to_vec(&Kept { user_id, allowance }) else {
                return;
            };
            tokio::task::spawn_blocking(move || {
                std::fs::create_dir_all(path.parent().unwrap_or(&path))?;
                sikemux_process::files::write_atomic(&path, &bytes, true)
            })
            .await
            .unwrap_or_else(|error| Err(std::io::Error::other(error)))
        }
        _ => return,
    };
    if let Err(error) = written {
        eprintln!("sikemux core: could not keep the account's allowance: {error}");
    }
    show(core).await;
}
