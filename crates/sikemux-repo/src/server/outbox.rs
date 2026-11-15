//! Pushes to a remote with an identity of the agents' own. No token an agent can reach may
//! write the repository, so every push there, an agent's or the core's, goes first to an
//! outbox: a bare repository the core makes for that one push, which borrows the checkout's
//! objects and holds the remote's branches and tags. Git pushes there as it would to the
//! remote, with its own checks and hooks. The outbox's hook then has the core forward exactly
//! those ref updates, from a git of its own that reads no config or hook an agent can write.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use sikemux_plugin_api::credential::{Ask, Credential, Reach};
use sikemux_process::sync::Locked;
use sikemux_process::vars;

use crate::cli::protocol::HarnessRequest;
use crate::git_guard::identity;
use crate::server::edits::workflows;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::{plugged, Core};

const FOLDER: &str = "outboxes";
const ZERO: &str = "0000000000000000000000000000000000000000";

struct Outbox {
    remote: String,
    checkout_objects: PathBuf,
}

static OUTBOXES: Mutex<Vec<(PathBuf, Outbox)>> = Mutex::new(Vec::new());
/// One forward at a time to each remote, so each sees the remote as the last one left it.
static REMOTES: Mutex<Vec<(String, Arc<tokio::sync::Mutex<()>>)>> = Mutex::new(Vec::new());
/// The core's own pushes in flight, by the name their git carries, with the key each minted.
static FORWARDS: Mutex<Vec<Forward>> = Mutex::new(Vec::new());

/// The fetches that fill an outbox, by the name their git carries, with the remote each reads.
static FETCHES: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

struct Forward {
    name: String,
    remote: String,
    minted: Option<Credential>,
}

/// Takes away the outboxes a core before this one left behind.
pub fn sweep(core: &Core) {
    if let Some(dir) = core.data_dir().as_ref().map(|dir| dir.join(FOLDER)) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Answers the helper git pushes through, and the outbox's hook, for the caller at `peer`.
pub async fn call(
    core: &Arc<Core>,
    request: &HarnessRequest,
    peer: SocketAddr,
    port: u16,
) -> Result<Value, String> {
    let params = &request.params;
    let text = |name: &str| params[name].as_str().map(str::to_owned);
    if super::credential::asker(core, request, peer, port)
        .await
        .is_none()
    {
        return Err("Sikemux pushes to this remote only for its own agents".to_owned());
    }
    match params["step"].as_str() {
        Some("open") => {
            let remote = text("remote").ok_or("remote is missing")?;
            let core = core.clone();
            let cwd = PathBuf::from(&request.project);
            let (outbox, _) = run_blocking(move || open(&core, &cwd, &remote)).await?;
            Ok(json!({ "outbox": outbox }))
        }
        Some("forward") => {
            let outbox = PathBuf::from(text("outbox").ok_or("outbox is missing")?);
            let updates = (params["updates"].as_array().into_iter().flatten())
                .filter_map(|update| {
                    let update = Update {
                        old: update["old"].as_str()?.to_owned(),
                        new: update["new"].as_str()?.to_owned(),
                        name: update["ref"].as_str()?.to_owned(),
                    };
                    update.sound().then_some(update)
                })
                .collect();
            let quarantine = text("quarantine").map(PathBuf::from);
            let (forwarded, said) = forward(core, &outbox, quarantine, updates).await;
            Ok(json!({ "forwarded": forwarded, "said": said }))
        }
        Some("close") => {
            let outbox = PathBuf::from(text("outbox").ok_or("outbox is missing")?);
            run_blocking(move || {
                close(&outbox);
                Ok(())
            })
            .await?;
            Ok(json!({}))
        }
        _ => Err("unknown step".to_owned()),
    }
}

/// The key the core's push named `name` may have for `remote`, minted on its git's first ask.
/// None for a name that is not a push of the core's.
pub(super) async fn minted(core: &Core, name: &str, remote: &str) -> Option<Credential> {
    let same = |forward: &Forward| forward.name == name && identity::same(&forward.remote, remote);
    let (url, minted) = {
        let forwards = FORWARDS.locked();
        let forward = forwards.iter().find(|forward| same(forward))?;
        (forward.remote.clone(), forward.minted.clone())
    };
    if minted.is_some() {
        return minted;
    }
    let ask = Ask {
        remote: url,
        reach: Reach::Core,
    };
    let credential = plugged::credential(core, ask).await.ok()??;
    if let Some(forward) = FORWARDS.locked().iter_mut().find(|forward| same(forward)) {
        forward.minted = Some(credential.clone());
    }
    Some(credential)
}

/// Whether `name` is the core's fetch into an outbox from `remote`.
pub(super) fn fetching(name: &str, remote: &str) -> bool {
    (FETCHES.locked().iter()).any(|(fetch, from)| fetch == name && identity::same(from, remote))
}

/// Deletes `branch` on `remote`, as long as it still holds `head`, through an outbox of its own.
pub(super) async fn delete_branch(
    core: &Arc<Core>,
    cwd: &Path,
    remote: &str,
    branch: &str,
    head: &str,
) -> (bool, String) {
    let opened = {
        let (core, cwd, remote) = (core.clone(), cwd.to_path_buf(), remote.to_owned());
        run_blocking(move || open(&core, &cwd, &remote)).await
    };
    let (outbox, remote) = match opened {
        Ok(opened) => opened,
        Err(said) => return (false, said),
    };
    let update = Update {
        old: head.to_owned(),
        new: ZERO.to_owned(),
        name: format!("refs/heads/{branch}"),
    };
    let went = match update.sound() {
        true => {
            let turn = turn_for(&remote);
            let _turn = turn.lock().await;
            send(core, &outbox, &remote, &[update]).await
        }
        false => (
            false,
            format!("Sikemux cannot delete a branch named {branch}"),
        ),
    };
    let _ = run_blocking(move || {
        close(&outbox);
        Ok(())
    })
    .await;
    went
}

/// Makes an outbox for a push from the checkout at `cwd` to `remote`, with the remote's
/// branches and tags fetched into it, and the address the core pushes to.
fn open(core: &Core, cwd: &Path, remote: &str) -> Result<(PathBuf, String), String> {
    let Some(remote) = identity::https_of(remote).filter(|https| identity::has(https)) else {
        return Err(format!(
            "Sikemux has no GitHub App or connector sign-in that pushes to {remote}"
        ));
    };
    let common = output(sikemux_git::git_command(cwd).args([
        "rev-parse",
        "--path-format=absolute",
        "--git-common-dir",
    ]))?;
    let checkout_objects = PathBuf::from(common.trim()).join("objects");
    let data = core
        .data_dir()
        .clone()
        .ok_or("Sikemux has no data folder")?;
    let outbox = data
        .join(FOLDER)
        .join(uuid::Uuid::new_v4().simple().to_string());
    let made = make(&outbox, &checkout_objects, &remote);
    if let Err(why) = made {
        let _ = std::fs::remove_dir_all(&outbox);
        return Err(why);
    }
    OUTBOXES.locked().push((
        outbox.clone(),
        Outbox {
            remote: remote.clone(),
            checkout_objects,
        },
    ));
    Ok((outbox, remote))
}

fn make(outbox: &Path, checkout_objects: &Path, remote: &str) -> Result<(), String> {
    let hook = identity::forward_hook().ok_or("Sikemux has no agent hooks folder")?;
    std::fs::create_dir_all(outbox).map_err(|error| error.to_string())?;
    output(clean(outbox).args(["init", "--quiet", "--bare", "--template="]))?;
    let hooks = outbox.join("hooks");
    std::fs::create_dir_all(&hooks).map_err(|error| error.to_string())?;
    write_alternates(outbox, &[checkout_objects])?;
    std::fs::write(hooks.join("pre-receive"), hook).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            hooks.join("pre-receive"),
            std::fs::Permissions::from_mode(0o755),
        )
        .map_err(|error| error.to_string())?;
    }
    let name = uuid::Uuid::new_v4().simple().to_string();
    FETCHES.locked().push((name.clone(), remote.to_owned()));
    let fetched = output(clean(outbox).env(vars::PUSH_GRANT, &name).args([
        "fetch",
        "--quiet",
        "--no-tags",
        "--no-write-fetch-head",
        remote,
        "+refs/heads/*:refs/heads/*",
        "+refs/tags/*:refs/tags/*",
    ]));
    FETCHES.locked().retain(|(fetch, _)| *fetch != name);
    fetched.map(|_| ())
}

struct Update {
    old: String,
    new: String,
    name: String,
}

impl Update {
    fn sound(&self) -> bool {
        let id = |text: &str| text.len() == 40 && text.bytes().all(|byte| byte.is_ascii_hexdigit());
        id(&self.old) && id(&self.new) && self.name.starts_with("refs/")
    }
}

/// Forwards `updates`, which git just pushed to `outbox`, to its remote, and what the remote
/// said. The outbox takes them only when the remote did.
async fn forward(
    core: &Arc<Core>,
    outbox: &Path,
    quarantine: Option<PathBuf>,
    updates: Vec<Update>,
) -> (bool, String) {
    let Some((remote, checkout_objects)) = (OUTBOXES.locked().iter())
        .find(|(path, _)| path == outbox)
        .map(|(_, kept)| (kept.remote.clone(), kept.checkout_objects.clone()))
    else {
        return (false, "Sikemux has no such push in flight".to_owned());
    };
    if quarantine
        .as_ref()
        .is_some_and(|quarantine| !quarantine.starts_with(outbox))
    {
        return (false, "Sikemux has no such push in flight".to_owned());
    }
    let turn = turn_for(&remote);
    let _turn = turn.lock().await;
    let core = core.clone();
    let outbox = outbox.to_path_buf();
    let handle = tokio::runtime::Handle::current();
    let went = tokio::task::spawn_blocking(move || {
        let mut objects = vec![checkout_objects.as_path()];
        objects.extend(quarantine.as_deref());
        if let Err(why) = write_alternates(&outbox, &objects) {
            return (false, why);
        }
        let went = handle.block_on(send(&core, &outbox, &remote, &updates));
        let _ = write_alternates(&outbox, &[&checkout_objects]);
        went
    })
    .await;
    went.unwrap_or_else(|_| (false, "Sikemux's forward of this push stopped".to_owned()))
}

/// Pushes `updates` from `outbox` to `remote` as the core, never forcing more than the agent's
/// own git did: a ref git moved off its old commit goes with a lease on that commit, any other
/// as a plain push the remote refuses when it moved on.
async fn send(core: &Arc<Core>, outbox: &Path, remote: &str, updates: &[Update]) -> (bool, String) {
    let moves: Vec<(String, String, String)> = (updates.iter())
        .map(|update| (update.name.clone(), update.old.clone(), update.new.clone()))
        .collect();
    let mut grant = None;
    let name = match workflows::forward(core, outbox, remote, &moves) {
        Err(said) => return (false, said),
        Ok(Some(passed)) => {
            let (_, name) = passed.env();
            grant = Some(passed);
            name
        }
        Ok(None) => {
            let name = uuid::Uuid::new_v4().simple().to_string();
            FORWARDS.locked().push(Forward {
                name: name.clone(),
                remote: remote.to_owned(),
                minted: None,
            });
            if minted(core, &name, remote).await.is_none() {
                FORWARDS.locked().retain(|forward| forward.name != name);
                return (
                    false,
                    format!("Sikemux could not get a key that pushes to {remote}. Check in Settings that its connector is signed in and reaches this repository, then push again."),
                );
            }
            name
        }
    };
    let mut push = clean(outbox);
    push.env(vars::PUSH_GRANT, &name)
        .args(["push", "--atomic", "--no-verify", remote]);
    for update in updates {
        if update.new == ZERO {
            push.arg(format!("--force-with-lease={}:{}", update.name, update.old));
            push.arg(format!(":{}", update.name));
        } else if update.old == ZERO || ancestor(outbox, &update.old, &update.new) {
            push.arg(format!("{}:{}", update.new, update.name));
        } else {
            push.arg(format!("--force-with-lease={}:{}", update.name, update.old));
            push.arg(format!("+{}:{}", update.new, update.name));
        }
    }
    let output = push.output();
    drop(grant);
    let forward = {
        let mut forwards = FORWARDS.locked();
        let at = forwards.iter().position(|forward| forward.name == name);
        at.map(|at| forwards.remove(at))
    };
    if let Some(credential) = forward.and_then(|forward| forward.minted) {
        if let Some(plugins) = plugged::provided(core) {
            if let Err(failure) = plugins.0.revoke(remote, credential.secret).await {
                eprintln!(
                    "sikemux core: a key that could push was not revoked, and lapses within the hour: {}",
                    plugged::explain(&failure)
                );
            }
        }
    }
    match output {
        Ok(output) => {
            let mut said = String::from_utf8_lossy(&output.stdout).into_owned();
            said.push_str(&String::from_utf8_lossy(&output.stderr));
            (output.status.success(), said)
        }
        Err(error) => (false, format!("Sikemux could not run git: {error}")),
    }
}

fn close(outbox: &Path) {
    let mut outboxes = OUTBOXES.locked();
    let Some(at) = outboxes.iter().position(|(path, _)| path == outbox) else {
        return;
    };
    outboxes.remove(at);
    drop(outboxes);
    let _ = std::fs::remove_dir_all(outbox);
}

fn turn_for(remote: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut remotes = REMOTES.locked();
    if let Some((_, turn)) = remotes
        .iter()
        .find(|(kept, _)| identity::same(kept, remote))
    {
        return turn.clone();
    }
    let turn = Arc::new(tokio::sync::Mutex::new(()));
    remotes.push((remote.to_owned(), turn.clone()));
    turn
}

fn ancestor(outbox: &Path, old: &str, new: &str) -> bool {
    (clean(outbox)
        .args(["merge-base", "--is-ancestor", old, new])
        .status())
    .is_ok_and(|status| status.success())
}

fn write_alternates(outbox: &Path, objects: &[&Path]) -> Result<(), String> {
    let text: String = (objects.iter())
        .map(|objects| format!("{}\n", objects.display()))
        .collect();
    std::fs::write(outbox.join("objects/info/alternates"), text).map_err(|error| error.to_string())
}

/// A git in `outbox` that reads only the outbox's own config, runs no hook, and asks the core
/// for its credential.
fn clean(outbox: &Path) -> Command {
    let mut command = sikemux_process::user_environment::command("git");
    command
        .env_clear()
        .envs(sikemux_process::user_environment::var_os("PATH").map(|path| ("PATH", path)))
        .env("HOME", outbox)
        .env("GIT_DIR", outbox)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .current_dir(outbox)
        .args(["-c", "core.hooksPath=/dev/null"]);
    sikemux_git::add_config(&mut command, &identity::credential_settings());
    command
}

fn output(command: &mut Command) -> Result<String, String> {
    let output = command.output().map_err(|error| error.to_string())?;
    match output.status.success() {
        true => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
        false => Err(String::from_utf8_lossy(&output.stderr).trim().to_owned()),
    }
}

async fn run_blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|_| Err("Sikemux stopped this push".to_owned()))
}
