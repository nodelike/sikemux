//! Repositories whose remote has an identity of the agents' own, such as a GitHub App. Git
//! that reaches such a remote asks the core for a credential that lasts one command, in place
//! of the person's keychain and SSH key, and the commits agents make there carry that identity
//! as committer. A remote the core pushes to with the person's own sign-in only has its pushes
//! go through the core. Every other remote works as it always has.
//!
//! The core keeps the list in a file beside the commit hooks, which the `git` and `gh`
//! stand-ins read, and hands the same settings to the git it runs itself. All of it reaches git
//! through `GIT_CONFIG_*` settings that apply by address, so a command that stays on this
//! machine starts nothing more.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sikemux_process::sync::Locked;

/// The name this binary is linked under for git to ask it for a credential.
pub(super) const HELPER: &str = "git-credential-sikemux";
const LIST: &str = "identities.json";
const ACTORS: &str = "actors";

/// Who commits made for the agents name as committer on a remote with an identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    pub name: String,
    pub email: String,
}

/// Who the core pushes to a remote as.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Acts {
    Agents(Actor),
    Person,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Acting {
    remote: String,
    acts: Acts,
}

impl Acting {
    fn actor(&self) -> Option<&Actor> {
        match &self.acts {
            Acts::Agents(actor) => Some(actor),
            Acts::Person => None,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct List {
    helper: String,
    /// Where git pushes to such a remote go instead: the helper, which hands them to the core.
    outbox: String,
    remotes: Vec<Acting>,
}

type Lookup = Box<dyn Fn(&str) -> Option<Acts> + Send + Sync>;

struct Kept {
    folder: PathBuf,
    endpoint: PathBuf,
    list: List,
}

static KEPT: Mutex<Option<Kept>> = Mutex::new(None);
static LOOKUP: OnceLock<Lookup> = OnceLock::new();

/// Keeps the list in `folder`, where the helper is linked, and has git ask the core at
/// `endpoint`. Picks up the list an earlier core left there.
pub(super) fn install(folder: &Path, endpoint: &Path) {
    let program = folder.join(HELPER);
    let helper = format!(
        "!{} {}",
        sikemux_process::single_quoted(&program.to_string_lossy()),
        sikemux_process::single_quoted(&endpoint.to_string_lossy()),
    );
    let outbox = format!(
        "ext::{} {} outbox %S",
        ext_quoted(&program.to_string_lossy()),
        ext_quoted(&endpoint.to_string_lossy()),
    );
    let remotes = read(folder).map(|list| list.remotes).unwrap_or_default();
    let kept = Kept {
        folder: folder.to_path_buf(),
        endpoint: endpoint.to_path_buf(),
        list: List {
            helper,
            outbox,
            remotes,
        },
    };
    sikemux_git::set_config_for_every_git(remote_settings(&kept.list));
    *KEPT.locked() = Some(kept);
}

/// How the core tells who it pushes to a remote as, with git's address for it, from what it
/// already knows and without asking over the network.
pub fn provide(lookup: impl Fn(&str) -> Option<Acts> + Send + Sync + 'static) {
    let _ = LOOKUP.set(Box::new(lookup));
}

/// Looks again at the remotes of the repository at `dir` and keeps the list right for them.
/// Whether one of them has an identity of the agents' own.
pub fn note(dir: &Path) -> bool {
    let Some(lookup) = LOOKUP.get() else {
        return false;
    };
    let urls = remote_urls(dir);
    let found: Vec<(String, Option<Acts>)> = urls
        .into_iter()
        .map(|url| {
            let acts = lookup(&url);
            (url, acts)
        })
        .collect();
    let acting = (found.iter()).any(|(_, acts)| matches!(acts, Some(Acts::Agents(_))));
    change(|remotes| {
        for (url, acts) in found {
            remotes.retain(|kept| kept.remote != url);
            if let Some(acts) = acts {
                remotes.push(Acting { remote: url, acts });
            }
        }
    });
    acting
}

/// Takes `remote` off the list, once the core has no credential for it, so its git works as
/// it did before.
pub fn forget(remote: &str) {
    let key = key_of(remote);
    change(|remotes| remotes.retain(|kept| key_of(&kept.remote) != key));
}

/// Whether the core pushes to `remote`, however its address is written.
pub fn has(remote: &str) -> bool {
    let key = key_of(remote);
    KEPT.locked()
        .as_ref()
        .is_some_and(|kept| (kept.list.remotes.iter()).any(|acting| key_of(&acting.remote) == key))
}

/// Whether `one` and `other` name the same repository, however each address is written.
pub fn same(one: &str, other: &str) -> bool {
    key_of(one) == key_of(other)
}

/// What a git the core starts in a folder of its own carries to ask the core for a credential.
pub fn credential_settings() -> Vec<(String, String)> {
    let Some(helper) = KEPT.locked().as_ref().map(|kept| kept.list.helper.clone()) else {
        return Vec::new();
    };
    vec![
        ("credential.helper".to_owned(), String::new()),
        ("credential.helper".to_owned(), helper),
        ("credential.useHttpPath".to_owned(), "true".to_owned()),
    ]
}

/// The pre-receive hook of a folder pushes are made to, which asks the core to forward them.
pub fn forward_hook() -> Option<String> {
    let kept = KEPT.locked();
    let kept = kept.as_ref()?;
    Some(format!(
        "#!/bin/sh\nexec {} {} forward\n",
        sikemux_process::single_quoted(&kept.folder.join(HELPER).to_string_lossy()),
        sikemux_process::single_quoted(&kept.endpoint.to_string_lossy()),
    ))
}

/// Whom the agents' commits in `dir` name as committer, when its remote has an identity.
pub fn actor_of(dir: &Path) -> Option<Actor> {
    let remotes = KEPT.locked().as_ref()?.list.remotes.clone();
    if remotes.is_empty() {
        return None;
    }
    let urls = remote_urls(dir);
    remotes
        .iter()
        .find(|kept| urls.contains(&kept.remote))
        .and_then(|kept| kept.actor().cloned())
}

/// What a commit the core makes for agents carries for its committer to be the identity of
/// the remote it goes to.
pub fn committer_settings() -> Vec<(String, String)> {
    match KEPT.locked().as_ref() {
        Some(kept) => committer_of(&kept.folder, &kept.list),
        None => Vec::new(),
    }
}

/// What an agent's own git carries, read from the list in `folder`. Nothing when no remote
/// has an identity.
pub(super) fn agent_settings(folder: &Path) -> Vec<(String, String)> {
    let Some(list) = read(folder) else {
        return Vec::new();
    };
    let mut settings = remote_settings(&list);
    settings.extend(committer_of(folder, &list));
    settings
}

/// The remote among `urls` with an identity on the list in `folder`, as its `https` address.
/// `urls` runs only when the list has one.
pub(super) fn acting_remote(folder: &Path, urls: impl FnOnce() -> Vec<String>) -> Option<String> {
    let list = read(folder)?;
    let urls = urls();
    let keys: Vec<String> = (list.remotes.iter())
        .filter(|kept| kept.actor().is_some())
        .map(|kept| key_of(&kept.remote))
        .collect();
    urls.iter()
        .find(|url| keys.contains(&key_of(url)))
        .and_then(|url| https_of(url))
}

fn read(folder: &Path) -> Option<List> {
    let text = std::fs::read(folder.join(LIST)).ok()?;
    serde_json::from_slice::<List>(&text)
        .ok()
        .filter(|list| !list.remotes.is_empty())
}

fn change(edit: impl FnOnce(&mut Vec<Acting>)) {
    let mut kept = KEPT.locked();
    let Some(kept) = kept.as_mut() else {
        return;
    };
    let before = kept.list.remotes.clone();
    edit(&mut kept.list.remotes);
    if kept.list.remotes == before {
        return;
    }
    if let Err(error) = write(kept) {
        eprintln!("sikemux core: could not keep the remotes agents have an identity on: {error}");
    }
    sikemux_git::set_config_for_every_git(remote_settings(&kept.list));
}

fn write(kept: &Kept) -> std::io::Result<()> {
    for actor in kept.list.remotes.iter().filter_map(Acting::actor) {
        let file = actor_file(&kept.folder, actor);
        if !file.is_file() {
            let text = format!(
                "[committer]\n\tname = {}\n\temail = {}\n",
                config_quoted(&actor.name),
                config_quoted(&actor.email),
            );
            sikemux_process::files::write_atomic(&file, text.as_bytes(), false)?;
        }
    }
    let list = serde_json::to_vec(&kept.list).map_err(std::io::Error::other)?;
    sikemux_process::files::write_atomic(&kept.folder.join(LIST), &list, false)
}

fn actor_file(folder: &Path, actor: &Actor) -> PathBuf {
    let name = Sha256::new()
        .chain_update(&actor.name)
        .chain_update([0])
        .chain_update(&actor.email)
        .finalize();
    folder
        .join(ACTORS)
        .join(format!("{}.gitconfig", hex::encode(&name[..8])))
}

fn config_quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// For each remote: git pushes through the core. With an identity of the agents' own, git also
/// asks only the core for its credential, never the person's keychain, and reaches the remote
/// over `https`, never the person's SSH key.
fn remote_settings(list: &List) -> Vec<(String, String)> {
    let mut settings = Vec::new();
    if !list.remotes.is_empty() {
        settings.push(("protocol.ext.allow".to_owned(), "user".to_owned()));
    }
    for acting in &list.remotes {
        let Some(https) = https_of(&acting.remote) else {
            continue;
        };
        let Some((host, path)) = host_and_path(&https) else {
            continue;
        };
        let ssh = [
            format!("git@{host}:{path}"),
            format!("ssh://git@{host}/{path}"),
        ];
        if acting.actor().is_some() {
            let credential = format!("credential.{https}");
            settings.extend([
                (format!("{credential}.helper"), String::new()),
                (format!("{credential}.helper"), list.helper.clone()),
                (format!("{credential}.useHttpPath"), "true".to_owned()),
            ]);
            for ssh in &ssh {
                settings.push((format!("url.{https}.insteadOf"), ssh.clone()));
            }
        }
        let pushed = format!("url.{} {https}.pushInsteadOf", list.outbox);
        for address in std::iter::once(&https).chain(&ssh) {
            settings.push((pushed.clone(), address.clone()));
        }
    }
    settings
}

/// `text` as one word of an `ext::` address, where a space or `%` would end or escape it.
fn ext_quoted(text: &str) -> String {
    text.replace('%', "%%").replace(' ', "% ")
}

fn committer_of(folder: &Path, list: &List) -> Vec<(String, String)> {
    (list.remotes.iter())
        .filter_map(|acting| {
            Some((
                format!("includeIf.hasconfig:remote.*.url:{}.path", acting.remote),
                actor_file(folder, acting.actor()?)
                    .to_string_lossy()
                    .into_owned(),
            ))
        })
        .collect()
}

/// The fetch and push addresses of the repository at `dir`'s remotes.
fn remote_urls(dir: &Path) -> Vec<String> {
    urls_from(sikemux_git::git_command(dir))
}

/// The remotes' addresses `git`, about to run in a repository, finds there.
pub(super) fn urls_from(mut git: std::process::Command) -> Vec<String> {
    let output = git
        .args(["config", "--get-regexp", r"^remote\..*\.(push)?url$"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    let mut urls: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_once(' ').map(|(_, url)| url.trim().to_owned()))
        .collect();
    urls.dedup();
    urls
}

/// `url`, a remote's address as git has it, as `https://host/path`. None for a folder or a
/// protocol git does not reach a code host over.
pub fn https_of(url: &str) -> Option<String> {
    let (host, path) = if let Some(rest) = url.strip_prefix("https://") {
        let (host, path) = rest.split_once('/')?;
        (host.rsplit('@').next()?, path)
    } else if let Some(rest) = url.strip_prefix("ssh://") {
        let (host, path) = rest.split_once('/')?;
        let host = host.rsplit('@').next()?;
        (host.split(':').next()?, path)
    } else if url.contains("://") {
        return None;
    } else {
        let (host, path) = url.split_once(':')?;
        if host.contains('/') || path.starts_with('/') {
            return None;
        }
        (host.rsplit('@').next()?, path)
    };
    (!host.is_empty() && !path.is_empty()).then(|| format!("https://{host}/{path}"))
}

fn host_and_path(https: &str) -> Option<(&str, &str)> {
    https.strip_prefix("https://")?.split_once('/')
}

/// The repository `url` names, the same for each way of writing its address.
fn key_of(url: &str) -> String {
    let https = https_of(url).unwrap_or_else(|| url.to_owned());
    let trimmed = https.trim_end_matches('/');
    trimmed
        .strip_suffix(".git")
        .unwrap_or(trimmed)
        .to_ascii_lowercase()
}
