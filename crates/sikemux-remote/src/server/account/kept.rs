//! An answer the account gave about itself, kept in the account folder with the account it is
//! for, so a host that cannot reach the account still knows it after a restart.

use std::path::Path;

use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::api::host_send_any;
use super::{folder, show};
use crate::server::remote_state::RemoteParts;
use crate::server::Core;

#[derive(Serialize, Deserialize)]
struct Kept<T> {
    user_id: String,
    answer: T,
}

/// What the account last answered in `file`, or nothing while `user_id` has not been answered.
pub async fn read<T: DeserializeOwned>(dir: &Path, file: &str, user_id: &str) -> Option<T> {
    let text = tokio::fs::read(dir.join(file)).await.ok()?;
    let kept: Kept<T> = serde_json::from_slice(&text).ok()?;
    (kept.user_id == user_id).then_some(kept.answer)
}

/// Asks the account `path`, keeps the answer in `file`, shows it and answers it. An account server
/// that does not know the question yet leaves nothing kept; one that cannot be reached leaves the
/// last answer. Either answers nothing.
pub async fn refresh<T: Serialize + DeserializeOwned>(
    core: &Core,
    path: &str,
    file: &str,
) -> Option<T> {
    let (Some(user_id), Some(dir)) = (core.remote().owner(), folder(core)) else {
        return None;
    };
    let Ok(answer) = host_send_any(core, Method::GET, path, None).await else {
        return None;
    };
    let kept_at = dir.join(file);
    let (written, fresh) = match answer.status() {
        StatusCode::NOT_FOUND => match tokio::fs::remove_file(&kept_at).await {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => (Err(error), None),
            _ => (Ok(()), None),
        },
        status if status.is_success() => {
            let answer = answer.json::<T>().await.ok()?;
            let kept = Kept { user_id, answer };
            let bytes = serde_json::to_vec(&kept).ok()?;
            let written = tokio::task::spawn_blocking(move || {
                std::fs::create_dir_all(kept_at.parent().unwrap_or(&kept_at))?;
                sikemux_process::files::write_atomic(&kept_at, &bytes, true)
            })
            .await
            .unwrap_or_else(|error| Err(std::io::Error::other(error)));
            (written, Some(kept.answer))
        }
        _ => return None,
    };
    if let Err(error) = written {
        eprintln!("sikemux core: could not keep the account's answer to {path}: {error}");
    }
    show(core).await;
    fresh
}
