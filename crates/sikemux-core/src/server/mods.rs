//! Mods: panels the person or their agents make as a web page. Each is a folder of the mods
//! folder holding an `index.html` and a `mod.json` that says what the mod is called and what it
//! reads and writes. The core lists them, follows their files and serves their pages.

mod bridge;
mod data;
pub use bridge::{BridgeParams, BridgeWrite};
pub use sikemux_calls::mods::{change_prompt, follows, make_prompt, Moved};

use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::parts::Parts;
pub use sikemux_calls::mods::{
    AllowParams, BadgeParams, ModExample, ModInfo, ModList, ModPage, ModParams, OffParams,
    PinModParams,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use sikemux_process::sync::Locked;

use super::changed::Topic;
use super::connection::blocking;
use super::kept::Kept;
use super::skill_library::{resolved, watch_target};
use super::{Core, CoreError, CoreResult};

pub(super) use bridge::{read, write};

/// The mods Sikemux ships for the person to copy into their mods folder and change: each
/// one's id, `mod.json` and `index.html`.
const EXAMPLES: [(&str, &str, &str); 1] = [(
    "pull-requests",
    include_str!("mods/examples/pull-requests/mod.json"),
    include_str!("mods/examples/pull-requests/index.html"),
)];

const MANIFEST: &str = "mod.json";
const SETTLED: &str = "mods.json";
const MAX_BADGE_CHARS: usize = 24;
const PAGE: &str = "index.html";
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_MODS: usize = 256;
const MAX_ID: usize = 64;
const MAX_NAME_CHARS: usize = 60;
const MAX_DESCRIPTION_CHARS: usize = 400;
const MAX_DECLARED: usize = 64;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    name: String,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    reads: Vec<String>,
    #[serde(default)]
    writes: Vec<String>,
}

/// What the person decided about the mods, kept in the data folder rather than beside the mods,
/// so a mod cannot decide it for itself.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settled {
    /// The writes the person allowed each mod, by id.
    #[serde(default)]
    allowed: BTreeMap<String, Vec<String>>,
    /// The project folder each pinned mod shows in, by id.
    #[serde(default)]
    pinned: BTreeMap<String, String>,
    #[serde(default)]
    off: BTreeSet<String>,
}

pub(crate) struct Mods {
    folder: Option<PathBuf>,
    settled: Mutex<Settled>,
    kept: Kept,
    badges: Mutex<BTreeMap<String, String>>,
    data: data::Store,
}

impl Mods {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let kept = Kept::new(data_dir, SETTLED);
        Self {
            folder: sikemux_process::paths::config_file("mods", ""),
            settled: Mutex::new(kept.load()),
            kept,
            badges: Mutex::default(),
            data: data::Store::new(data_dir),
        }
    }

    /// Whether the person allowed `writes`, all a mod declares, for the mod `id`.
    fn allowed(&self, id: &str, writes: &[String]) -> bool {
        writes.is_empty()
            || self
                .settled
                .locked()
                .allowed
                .get(id)
                .is_some_and(|allowed| allowed == writes)
    }

    fn is_off(&self, id: &str) -> bool {
        self.settled.locked().off.contains(id)
    }

    fn settle(&self, change: impl FnOnce(&mut Settled)) {
        let mut settled = self.settled.locked();
        change(&mut settled);
        self.kept.save_soon(&*settled);
    }

    fn folder(&self) -> CoreResult<PathBuf> {
        self.folder
            .clone()
            .ok_or_else(|| CoreError::from("this host has no folder for mods"))
    }
}

/// Whether `id` can name a mod's folder: lowercase letters, digits and dashes.
pub fn is_mod_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn read_manifest(folder: &Path) -> Result<Manifest, String> {
    let path = folder.join(MANIFEST);
    let size = std::fs::metadata(&path)
        .map_err(|_| format!("it has no {MANIFEST}"))?
        .len();
    if size > MAX_MANIFEST_BYTES {
        return Err(format!("its {MANIFEST} is over 64 KiB"));
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("its {MANIFEST} could not be read: {error}"))?;
    let mut manifest: Manifest = serde_json::from_str(&text)
        .map_err(|error| format!("its {MANIFEST} does not read: {error}"))?;
    manifest.name = manifest.name.trim().chars().take(MAX_NAME_CHARS).collect();
    if manifest.name.is_empty() {
        return Err(format!("its {MANIFEST} gives no name"));
    }
    manifest.description = manifest
        .description
        .trim()
        .chars()
        .take(MAX_DESCRIPTION_CHARS)
        .collect();
    for declared in [&mut manifest.reads, &mut manifest.writes] {
        declared.retain(|name| !name.trim().is_empty());
        declared.truncate(MAX_DECLARED);
        declared.sort();
        declared.dedup();
    }
    Ok(manifest)
}

fn info(id: String, folder: &Path) -> ModInfo {
    let read = read_manifest(folder).and_then(|manifest| match folder.join(PAGE).is_file() {
        true => Ok(manifest),
        false => Err(format!("it has no {PAGE}")),
    });
    match read {
        Ok(manifest) => ModInfo {
            id,
            name: manifest.name,
            icon: manifest.icon,
            description: manifest.description,
            reads: manifest.reads,
            writes: manifest.writes,
            problem: None,
            allowed: false,
            badge: None,
            project: None,
            off: false,
        },
        Err(problem) => ModInfo {
            name: id.clone(),
            id,
            icon: None,
            description: String::new(),
            reads: Vec::new(),
            writes: Vec::new(),
            problem: Some(problem),
            allowed: false,
            badge: None,
            project: None,
            off: false,
        },
    }
}

fn list_in(folder: &Path) -> Vec<ModInfo> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut mods: Vec<ModInfo> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let id = entry.file_name().into_string().ok()?;
            is_mod_id(&id).then(|| info(id, &entry.path()))
        })
        .collect();
    mods.sort_by_key(|shown| shown.name.to_lowercase());
    mods.truncate(MAX_MODS);
    mods
}

pub(super) async fn list(core: &Arc<Core>) -> CoreResult<ModList> {
    let folder = core.mods().folder()?;
    let mut mods = blocking(move || {
        let examples = EXAMPLES
            .iter()
            .filter_map(|(id, manifest, _)| {
                let manifest: Manifest = serde_json::from_str(manifest).ok()?;
                Some(ModExample {
                    id: (*id).to_owned(),
                    name: manifest.name,
                    description: manifest.description,
                    added: folder.join(id).exists(),
                })
            })
            .collect();
        Ok(ModList {
            mods: list_in(&folder),
            folder,
            examples,
        })
    })
    .await?;
    let badges = core.mods().badges.locked();
    for shown in &mut mods.mods {
        shown.allowed = core.mods().allowed(&shown.id, &shown.writes);
        shown.badge = badges.get(&shown.id).cloned();
        let settled = core.mods().settled.locked();
        shown.project = settled.pinned.get(&shown.id).cloned();
        shown.off = settled.off.contains(&shown.id);
    }
    Ok(mods)
}

/// Every client lists the mods again, with no mod's page read again.
fn settled_changed(core: &Core) {
    core.changed(What::ReadIn(Topic::Mods, None, Some(Vec::new())));
}

/// Keeps what the person decided about a mod's writes. An approval holds only for the writes
/// the mod declares now, so one shown before its `mod.json` changed is turned down.
pub(super) async fn allow(core: &Arc<Core>, params: AllowParams) -> CoreResult<()> {
    match params.writes {
        Some(mut writes) => {
            let folder = mod_folder(core, &params.id)?;
            let manifest =
                blocking(move || read_manifest(&folder).map_err(CoreError::from)).await?;
            writes.sort();
            writes.dedup();
            if writes != manifest.writes {
                return Err("this mod's writes changed since; open it again to be asked".into());
            }
            core.mods().settle(|settled| {
                settled.allowed.insert(params.id, writes);
            });
        }
        None => core.mods().settle(|settled| {
            settled.allowed.remove(&params.id);
        }),
    }
    settled_changed(core);
    Ok(())
}

/// Keeps the few words a mod shows beside its name until the core stops.
pub(super) fn set_badge(core: &Core, params: BadgeParams) -> CoreResult<()> {
    if !is_mod_id(&params.id) {
        return Err("that is not a mod's id".into());
    }
    let badge = params
        .badge
        .map(|badge| {
            badge
                .trim()
                .chars()
                .take(MAX_BADGE_CHARS)
                .collect::<String>()
        })
        .filter(|badge| !badge.is_empty());
    let mut badges = core.mods().badges.locked();
    let moved = match badge {
        Some(badge) if badges.len() < MAX_MODS || badges.contains_key(&params.id) => {
            badges.insert(params.id, badge.clone()) != Some(badge)
        }
        Some(_) => false,
        None => badges.remove(&params.id).is_some(),
    };
    drop(badges);
    if moved {
        settled_changed(core);
    }
    Ok(())
}

/// A mod's page with every local picture and font it names inlined, so a client on another
/// host shows it as this one does.
pub(super) async fn page(core: &Arc<Core>, params: ModParams) -> CoreResult<ModPage> {
    let folder = shown_folder(core, &params.id)?;
    blocking(move || {
        if let Some(problem) = info(params.id, &folder).problem {
            return Err(CoreError::from(format!("this mod cannot show: {problem}")));
        }
        let html = crate::pages::build(&folder.join(PAGE))?;
        Ok(ModPage { html })
    })
    .await
}

fn mod_folder(core: &Core, id: &str) -> CoreResult<PathBuf> {
    if !is_mod_id(id) {
        return Err("that is not a mod's id".into());
    }
    Ok(core.mods().folder()?.join(id))
}

/// The name of the mod `id`, when it can show, for an agent opening it.
pub(super) async fn title(core: &Core, id: &str) -> Result<String, String> {
    let folder = shown_folder(core, id).map_err(|error| error.to_string())?;
    let id = id.to_owned();
    blocking(move || {
        let shown = info(id, &folder);
        Ok(match (folder.is_dir(), shown.problem) {
            (false, _) => Err(format!("there is no mod in {}", folder.display())),
            (true, Some(problem)) => Err(format!("this mod cannot show: {problem}")),
            (true, None) => Ok(shown.name),
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

/// The folder of a mod that is on, which alone shows and reads.
fn shown_folder(core: &Core, id: &str) -> CoreResult<PathBuf> {
    match core.mods().is_off(id) {
        true => Err("this mod is turned off; Settings, Mods turns it on".into()),
        false => mod_folder(core, id),
    }
}

pub(super) fn pin(core: &Core, params: PinModParams) -> CoreResult<()> {
    mod_folder(core, &params.id)?;
    core.mods().settle(|settled| match params.project {
        Some(project) => {
            settled.pinned.insert(params.id, project);
        }
        None => {
            settled.pinned.remove(&params.id);
        }
    });
    settled_changed(core);
    Ok(())
}

pub(super) fn turn_off(core: &Core, params: OffParams) -> CoreResult<()> {
    mod_folder(core, &params.id)?;
    core.mods().settle(|settled| {
        match params.off {
            true => settled.off.insert(params.id.clone()),
            false => settled.off.remove(&params.id),
        };
    });
    core.changed(What::ReadIn(Topic::Mods, None, Some(vec![params.id])));
    Ok(())
}

/// Copies the example `id` into the mods folder, where the person can change it.
pub(super) async fn add_example(core: &Arc<Core>, params: ModParams) -> CoreResult<()> {
    let (_, manifest, page) = EXAMPLES
        .iter()
        .find(|(id, _, _)| *id == params.id)
        .ok_or_else(|| CoreError::from("Sikemux has no example mod by that id"))?;
    let folder = mod_folder(core, &params.id)?;
    blocking(move || {
        if folder.exists() {
            return Err(CoreError::from(format!(
                "{} is there already",
                folder.display()
            )));
        }
        std::fs::create_dir_all(&folder)?;
        std::fs::write(folder.join(PAGE), page)?;
        std::fs::write(folder.join(MANIFEST), manifest)?;
        Ok(())
    })
    .await
}

/// Moves a mod's folder to the Trash and forgets its data and what the person decided about it.
pub(super) async fn remove(core: &Arc<Core>, params: ModParams) -> CoreResult<()> {
    let folder = mod_folder(core, &params.id)?;
    let (worker, id) = (core.clone(), params.id.clone());
    blocking(move || {
        trash::delete(&folder).map_err(|error| CoreError::from(error.to_string()))?;
        worker.mods().data.clear(&id)
    })
    .await?;
    core.mods().settle(|settled| {
        settled.allowed.remove(&params.id);
        settled.pinned.remove(&params.id);
        settled.off.remove(&params.id);
    });
    core.mods().badges.locked().remove(&params.id);
    settled_changed(core);
    Ok(())
}

/// The mods whose files moved, by the paths a change names under `folder`.
fn touched(folder: &Path, paths: &[PathBuf]) -> BTreeSet<String> {
    paths
        .iter()
        .filter_map(|path| path.strip_prefix(folder).ok())
        .filter_map(|rest| rest.components().next())
        .filter_map(|first| first.as_os_str().to_str())
        .filter(|id| is_mod_id(id))
        .map(str::to_owned)
        .collect()
}

/// Follows the mods folder while the core runs, including before it is made, and tells every
/// client which mods changed, so a list reads again and an open mod shows its new page.
pub(crate) fn watch(core: &Arc<Core>) {
    let Some(folder) = core.mods().folder.clone() else {
        return;
    };
    let core = Arc::downgrade(core);
    std::thread::Builder::new()
        .name("sikemux-mods".into())
        .spawn(move || {
            let real = resolved(&folder);
            let (changed, changes) = std::sync::mpsc::channel::<Vec<PathBuf>>();
            let concerned = real.clone();
            let Ok(mut watcher) =
                notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                    if let Ok(event) = event {
                        let paths: Vec<PathBuf> = event
                            .paths
                            .into_iter()
                            .filter(|path| {
                                path.starts_with(&concerned) || concerned.starts_with(path)
                            })
                            .collect();
                        if !paths.is_empty() {
                            let _ = changed.send(paths);
                        }
                    }
                })
            else {
                return;
            };
            let mut watching = None;
            let mut follow = |watcher: &mut notify::RecommendedWatcher| {
                let target = watch_target(&folder);
                if watching.as_ref() != Some(&target)
                    && watcher.watch(&target, RecursiveMode::Recursive).is_ok()
                {
                    if let Some(old) = watching.replace(target) {
                        let _ = watcher.unwatch(&old);
                    }
                }
            };
            follow(&mut watcher);
            while let Ok(first) = changes.recv() {
                std::thread::sleep(Duration::from_millis(150));
                let mut paths = first;
                while let Ok(more) = changes.try_recv() {
                    paths.extend(more);
                }
                follow(&mut watcher);
                let Some(core) = core.upgrade() else {
                    return;
                };
                let ids: Vec<String> = touched(&real, &paths).into_iter().collect();
                core.changed(What::ReadIn(Topic::Mods, None, Some(ids)));
            }
        })
        .ok();
}

impl super::part::Part for Mods {
    fn start(&self, core: &Arc<Core>) -> Vec<tokio::task::JoinHandle<()>> {
        watch(core);
        Vec::new()
    }

    fn flush(&self) {
        self.kept.flush();
    }
}
