// Reading an app's tracks, and releasing an uploaded bundle on one.

use std::path::Path;

use serde::Deserialize;
use serde_json::{json, Value};
use sikemux_plugin_api::json::{at, text};

use crate::auth::expand;
use crate::client::Edit;
use crate::error::{PlayError, PlayResult};

#[derive(Deserialize)]
pub struct TracksAsk {
    pub package: String,
}

/// Reads through an edit that is dropped after, since Play keeps tracks only inside one.
pub async fn tracks(data_dir: &Path, ask: TracksAsk) -> PlayResult<Value> {
    let edit = Edit::open(data_dir, &ask.package).await?;
    let read = edit.get("/tracks").await;
    edit.discard().await;
    let tracks: Vec<Value> = at(&read?, "/tracks")
        .as_array()
        .into_iter()
        .flatten()
        .map(|track| {
            let releases: Vec<Value> = at(track, "/releases")
                .as_array()
                .into_iter()
                .flatten()
                .map(|release| {
                    json!({
                        "name": text(release, "/name"),
                        "versionCodes": at(release, "/versionCodes"),
                        "status": text(release, "/status"),
                        "userFraction": at(release, "/userFraction"),
                    })
                })
                .collect();
            json!({ "track": text(track, "/track"), "releases": releases })
        })
        .collect();
    Ok(json!({ "package": ask.package, "tracks": tracks }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadAsk {
    pub package: String,
    pub file: String,
    #[serde(default)]
    pub track: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub release_notes: Option<String>,
}

pub async fn upload(data_dir: &Path, ask: UploadAsk) -> PlayResult<Value> {
    let file = expand(&ask.file);
    if file.extension().and_then(|end| end.to_str()) != Some("aab") || !file.is_file() {
        return Err(PlayError::BadArg(format!(
            "no .aab at {}; Play takes an app bundle",
            file.display()
        )));
    }
    let track = ask.track.unwrap_or_else(|| "internal".into());
    let status = ask.status.unwrap_or_else(|| "completed".into());
    let edit = Edit::open(data_dir, &ask.package).await?;
    let released = release(&edit, &file, &track, &status, ask.name, ask.release_notes).await;
    match released {
        Ok(version) => Ok(json!({
            "package": ask.package,
            "track": track,
            "status": status,
            "versionCode": version,
        })),
        Err(error) => {
            edit.discard().await;
            Err(error)
        }
    }
}

async fn release(
    edit: &Edit,
    file: &Path,
    track: &str,
    status: &str,
    name: Option<String>,
    notes: Option<String>,
) -> PlayResult<Value> {
    let bundle = edit.upload_bundle(file).await?;
    let version = at(&bundle, "/versionCode").clone();
    if version.is_null() {
        return Err(PlayError::Response(
            "Play did not say the bundle's version code".into(),
        ));
    }
    let mut release = serde_json::Map::new();
    release.insert(
        "versionCodes".into(),
        json!([version.to_string().trim_matches('"')]),
    );
    release.insert("status".into(), json!(status));
    if let Some(name) = name {
        release.insert("name".into(), json!(name));
    }
    if let Some(notes) = notes {
        release.insert(
            "releaseNotes".into(),
            json!([{ "language": "en-US", "text": notes }]),
        );
    }
    edit.put(
        &format!("/tracks/{track}"),
        json!({ "track": track, "releases": [release] }),
    )
    .await?;
    edit.commit().await?;
    Ok(version)
}
