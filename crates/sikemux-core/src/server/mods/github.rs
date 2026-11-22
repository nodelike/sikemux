//! Installing a mod from a GitHub repository, as a `sikemux://install-mod` link asks: first what
//! its `mod.json` says at the repository's newest commit, for the person to read, then, once they
//! agree, every file of that same commit into the mods folder.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use sikemux_calls::mods::{github_repo, InstallRepoParams, RepoMod, RepoParams};
use sikemux_process::sync::Locked;

use super::folder::{self, fits};
use super::{is_mod_id, parse_manifest, settled_changed, Core, CoreError, CoreResult};
use super::{MANIFEST, MAX_ID, MAX_MANIFEST_BYTES, PAGE};
use crate::server::connection::blocking;
use crate::server::parts::Parts;

const API: &str = "https://api.github.com";
const RAW: &str = "https://raw.githubusercontent.com";
const TIMEOUT: Duration = Duration::from_secs(20);
const MOST_FILES: usize = 200;
const MOST_FILE_BYTES: usize = 4 * 1024 * 1024;
const MOST_TREE_BYTES: usize = 2 * 1024 * 1024;
const MOST_MOD_BYTES: u64 = 16 * 1024 * 1024;
const DOWNLOADS_AT_ONCE: usize = 6;

fn client() -> CoreResult<&'static Client> {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    crate::http::shared(&CLIENT, || crate::http::builder().timeout(TIMEOUT))
        .map_err(|error| CoreError::from(error.to_string()))
}

/// The answer's bytes, up to `most`, with what went wrong in words the person can act on.
async fn fetch(url: &str, accept: &str, most: usize, what: &str) -> CoreResult<Vec<u8>> {
    let response = (client()?.get(url).header("Accept", accept).send().await)
        .map_err(|_| CoreError::from("GitHub could not be reached"))?;
    match response.status() {
        status if status.is_success() => {}
        StatusCode::NOT_FOUND => return Err(format!("GitHub has no {what}").into()),
        StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS => {
            return Err("GitHub is limiting requests from here; try again in a while".into())
        }
        status => return Err(format!("GitHub answered {status} for {what}").into()),
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| CoreError::from("GitHub stopped answering"))?;
        if body.len() + chunk.len() > most {
            return Err(format!("{what} is too large to install").into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn repo_of(text: &str) -> CoreResult<String> {
    github_repo(text).ok_or_else(|| "that is not a GitHub repository such as owner/name".into())
}

fn is_commit(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

async fn newest_commit(repo: &str) -> CoreResult<String> {
    let url = format!("{API}/repos/{repo}/commits/HEAD");
    let body = fetch(&url, "application/vnd.github.sha", 64, "such repository").await?;
    let commit = String::from_utf8_lossy(&body).trim().to_ascii_lowercase();
    match is_commit(&commit) {
        true => Ok(commit),
        false => Err("GitHub did not say which commit the repository is at".into()),
    }
}

/// `path` at `commit` of `repo`, as raw bytes.
async fn file(repo: &str, commit: &str, path: &str, most: usize) -> CoreResult<Vec<u8>> {
    let mut url = url::Url::parse(RAW).map_err(|error| CoreError::from(error.to_string()))?;
    url.path_segments_mut()
        .map_err(|_| CoreError::from("bad address"))?
        .extend(repo.split('/'))
        .push(commit)
        .extend(path.split('/'));
    fetch(url.as_str(), "*/*", most, path).await
}

#[derive(Deserialize)]
struct Tree {
    tree: Vec<Node>,
    #[serde(default)]
    truncated: bool,
}

#[derive(Deserialize)]
struct Node {
    path: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    mode: String,
    #[serde(default)]
    size: u64,
}

/// The files of `commit` a mod's folder keeps, by path: plain files only, none under a dot-folder
/// such as `.github`, nor a link or a submodule.
async fn files(repo: &str, commit: &str) -> CoreResult<Vec<(String, u64)>> {
    let url = format!("{API}/repos/{repo}/git/trees/{commit}?recursive=1");
    let body = fetch(
        &url,
        "application/vnd.github+json",
        MOST_TREE_BYTES,
        "list of its files",
    )
    .await?;
    let tree: Tree = serde_json::from_slice(&body)
        .map_err(|_| CoreError::from("GitHub's list of its files did not read"))?;
    if tree.truncated {
        return Err("the repository has too many files to be a mod".into());
    }
    let files: Vec<(String, u64)> = tree
        .tree
        .into_iter()
        .filter(|node| node.kind == "blob" && matches!(node.mode.as_str(), "100644" | "100755"))
        .filter(|node| fits(&node.path))
        .map(|node| (node.path, node.size))
        .collect();
    let total: u64 = files.iter().map(|(_, size)| size).sum();
    if files.len() > MOST_FILES || total > MOST_MOD_BYTES {
        return Err(format!(
            "the repository holds more than a mod should: {} files, {} MB",
            files.len(),
            total.div_ceil(1024 * 1024)
        )
        .into());
    }
    if let Some((path, _)) = files
        .iter()
        .find(|(_, size)| *size > MOST_FILE_BYTES as u64)
    {
        return Err(format!("{path} is too large to install").into());
    }
    for needed in [MANIFEST, PAGE] {
        if !files.iter().any(|(path, _)| path == needed) {
            return Err(
                format!("the repository has no {needed} at its top, so it is not a mod").into(),
            );
        }
    }
    Ok(files)
}

/// `text` as much of a mod's id as it can make: lowercase letters, digits and single dashes.
fn slug(text: &str) -> String {
    let lowered: String = text
        .to_ascii_lowercase()
        .chars()
        .map(|letter| match letter.is_ascii_alphanumeric() {
            true => letter,
            false => '-',
        })
        .collect();
    let mut slug = lowered
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    slug.truncate(MAX_ID);
    slug.trim_end_matches('-').to_owned()
}

/// The folder `repo` installs into, and whether an earlier install of it is there: its name,
/// or its owner's and its name when another mod has that, so no mod of the person's is replaced.
fn landing(core: &Core, repo: &str) -> CoreResult<(String, bool)> {
    let mods = core.mods().folder()?;
    let (owner, name) = repo.split_once('/').unwrap_or_default();
    let sources = core.mods().settled.locked().sources.clone();
    let candidates = [slug(name), slug(&format!("{name}-{owner}"))];
    for id in candidates.into_iter().filter(|id| is_mod_id(id)) {
        let ours = sources
            .get(&id)
            .is_some_and(|source| source.eq_ignore_ascii_case(repo));
        let there = mods.join(&id).exists();
        if ours || !there {
            return Ok((id, ours && there));
        }
    }
    Err(
        format!("your mods folder already has mods named {name}; remove one to install this")
            .into(),
    )
}

/// What the mod in `repo` says of itself at its newest commit, with nothing of it downloaded but
/// its `mod.json`.
pub(in crate::server) async fn preview(
    core: &Arc<Core>,
    params: RepoParams,
) -> CoreResult<RepoMod> {
    let repo = repo_of(&params.repo)?;
    let commit = newest_commit(&repo).await?;
    files(&repo, &commit).await?;
    let text = file(&repo, &commit, MANIFEST, MAX_MANIFEST_BYTES as usize).await?;
    let manifest = parse_manifest(&String::from_utf8_lossy(&text)).map_err(CoreError::from)?;
    let (id, replaces) = landing(core, &repo)?;
    Ok(RepoMod {
        repo,
        commit,
        id,
        name: manifest.name,
        icon: manifest.icon,
        description: manifest.description,
        reads: manifest.reads,
        writes: manifest.writes,
        connect: manifest.connect,
        collections: manifest.collections.into_keys().collect(),
        replaces,
    })
}

/// Downloads every file of the commit the person was shown into a new folder, checks it reads as
/// a mod, then puts it in place, sending an earlier install of the same repository to the Trash.
/// What it may write or reach still waits for the person to allow it, as any mod's does.
pub(in crate::server) async fn install(
    core: &Arc<Core>,
    params: InstallRepoParams,
) -> CoreResult<String> {
    let repo = repo_of(&params.repo)?;
    let commit = params.commit.to_ascii_lowercase();
    if !is_commit(&commit) {
        return Err("that is not a commit".into());
    }
    let (id, _) = landing(core, &repo)?;
    let mods = core.mods().folder()?;
    let listed = files(&repo, &commit).await?;
    let fetched: Vec<(String, Vec<u8>)> = futures_util::stream::iter(listed)
        .map(|(path, _)| async {
            let bytes = file(&repo, &commit, &path, MOST_FILE_BYTES).await?;
            CoreResult::Ok((path, bytes))
        })
        .buffered(DOWNLOADS_AT_ONCE)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<CoreResult<_>>()?;
    let placed = id.clone();
    blocking(move || {
        std::fs::create_dir_all(&mods)?;
        let staged = folder::staging(&mods, &placed)?;
        let built = (|| {
            for (path, bytes) in &fetched {
                folder::put(&staged, path, bytes).map_err(|error| error.to_string())?;
            }
            super::read_manifest(&staged)?;
            folder::swap(&mods, &placed, &staged, true)
        })();
        if built.is_err() {
            let _ = std::fs::remove_dir_all(&staged);
        }
        built.map_err(|problem| CoreError::from(format!("it did not install: {problem}")))
    })
    .await?;
    core.mods().settle(|settled| {
        settled.sources.insert(id.clone(), repo);
        settled.allowed.remove(&id);
        settled.connects.remove(&id);
    });
    settled_changed(core);
    Ok(id)
}
