//! The host's skills: the Sikemux library every agent here is handed, and
//! the person's own global skills beside it, which Sikemux never changes
//! unless asked to move one in. Any client manages them through these calls.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sikemux_process::sync::Locked;

use super::changed::Topic;
use crate::agent_catalog::AgentKind;
use crate::skills::{self, Skill};

use super::connection::blocking;
use super::{Core, CoreError, CoreResult};

/// The most global skills kept as newly added until the person answers.
const MAX_ADDED: usize = 64;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OwnSkills {
    agent: &'static str,
    folder: PathBuf,
    skills: Vec<Skill>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillsOverview {
    library: PathBuf,
    skills: Vec<Skill>,
    own: Vec<OwnSkills>,
    /// Names in the library that one of the person's own skills also uses.
    clashes: Vec<String>,
}

/// A skill that appeared in an agent's own global folder while the core ran,
/// most likely written there by that agent's own skill maker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct GlobalSkillAdded {
    agent: &'static str,
    name: String,
    path: PathBuf,
}

/// The global skills that appeared since the core started, until moved in.
fn newly_added() -> std::sync::MutexGuard<'static, Vec<GlobalSkillAdded>> {
    static ADDED: Mutex<Vec<GlobalSkillAdded>> = Mutex::new(Vec::new());
    ADDED.locked()
}

/// Each agent's own global skills folder, with the agents that read it,
/// whether or not it exists yet. These belong to the person; Sikemux only reads them.
fn global_folders() -> Vec<(&'static str, PathBuf)> {
    let Some(home) = sikemux_process::paths::home() else {
        return Vec::new();
    };
    let mut folders: Vec<(&'static str, PathBuf)> = AgentKind::ALL
        .into_iter()
        .filter_map(|kind| Some((kind.entry().label, kind.ops().skills_folder()?)))
        .collect();
    folders.push(("Codex and other agents", home.join(".agents/skills")));
    let mut seen = Vec::new();
    folders.retain(|(_, folder)| {
        let real = std::fs::canonicalize(folder).unwrap_or_else(|_| folder.clone());
        let fresh = !seen.contains(&real);
        seen.push(real);
        fresh
    });
    folders
}

fn library_overview(data_dir: &Path, folders: Vec<(&'static str, PathBuf)>) -> SkillsOverview {
    let library = skills::list(data_dir);
    let own: Vec<OwnSkills> = folders
        .into_iter()
        .map(|(agent, folder)| OwnSkills {
            agent,
            skills: skills::list_in(&folder),
            folder,
        })
        .filter(|own| !own.skills.is_empty())
        .collect();
    let clashes = library
        .iter()
        .filter(|skill| {
            own.iter()
                .any(|own| own.skills.iter().any(|theirs| theirs.name == skill.name))
        })
        .map(|skill| skill.name.clone())
        .collect();
    SkillsOverview {
        library: skills::library(data_dir),
        skills: library,
        own,
        clashes,
    }
}

/// Moves one of the person's own skills into the library. Only a skill that
/// sits directly in a known global skills folder can move.
fn adopt_global(
    data_dir: &Path,
    folders: &[(&'static str, PathBuf)],
    path: &Path,
) -> Result<Skill, String> {
    let known = folders
        .iter()
        .any(|(_, folder)| path.parent() == Some(folder.as_path()));
    if !known {
        return Err(format!(
            "{} is not in an agent's skills folder",
            path.display()
        ));
    }
    let skill = skills::install_folder(data_dir, path)?;
    std::fs::remove_dir_all(path).map_err(|error| {
        format!(
            "{} is in Sikemux now, but its old folder stayed: {error}",
            skill.name
        )
    })?;
    Ok(skill)
}

#[derive(Deserialize)]
pub(super) struct NewSkill {
    name: String,
    description: String,
    body: String,
}

#[derive(Deserialize)]
pub(super) struct Source {
    source: String,
    subfolder: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct Named {
    name: String,
}

#[derive(Deserialize)]
pub(super) struct Located {
    path: PathBuf,
}

/// Runs `work` on the library in the data folder, off the async threads.
async fn on_library<T: Serialize + Send + 'static>(
    core: &Core,
    work: impl FnOnce(&Path) -> CoreResult<T> + Send + 'static,
) -> CoreResult<Value> {
    let data_dir = core
        .workspaces
        .data_dir()
        .ok_or_else(|| CoreError::from("this host keeps no skills"))?
        .to_path_buf();
    blocking(move || Ok(serde_json::to_value(work(&data_dir)?)?)).await
}

pub(super) async fn overview(core: &Core) -> CoreResult<Value> {
    on_library(core, |data_dir| {
        Ok(library_overview(data_dir, global_folders()))
    })
    .await
}

pub(super) async fn added(core: &Core) -> CoreResult<Value> {
    on_library(core, |_| Ok(newly_added().clone())).await
}

pub(super) async fn create(core: &Core, skill: NewSkill) -> CoreResult<Value> {
    on_library(core, move |data_dir| {
        Ok(skills::create(
            data_dir,
            &skill.name,
            &skill.description,
            &skill.body,
        )?)
    })
    .await
}

pub(super) async fn install(core: &Core, from: Source) -> CoreResult<Value> {
    on_library(core, move |data_dir| {
        let source = from.source.trim();
        Ok(if source.starts_with('/') {
            skills::install_folder(data_dir, Path::new(source))?
        } else {
            let subfolder = from
                .subfolder
                .as_deref()
                .map(str::trim)
                .filter(|path| !path.is_empty());
            skills::install_git(data_dir, source, subfolder)?
        })
    })
    .await
}

pub(super) async fn remove(core: &Core, named: Named) -> CoreResult<Value> {
    on_library(core, move |data_dir| {
        Ok(skills::remove(data_dir, &named.name)?)
    })
    .await
}

pub(super) async fn adopt(core: &Core, located: Located) -> CoreResult<Value> {
    on_library(core, move |data_dir| {
        let skill = adopt_global(data_dir, &global_folders(), &located.path)?;
        newly_added().retain(|added| added.path != located.path);
        Ok(skill)
    })
    .await
}

fn added_skills(
    folders: &[(&'static str, PathBuf)],
    known: &mut HashSet<PathBuf>,
) -> Vec<GlobalSkillAdded> {
    let mut added = Vec::new();
    for (agent, folder) in folders {
        for skill in skills::list_in(folder) {
            if known.insert(skill.path.clone()) {
                added.push(GlobalSkillAdded {
                    agent,
                    name: skill.name,
                    path: skill.path,
                });
            }
        }
    }
    added
}

/// A folder's real path, also for one not made yet: macOS reports changes
/// under real paths, and a home or config folder may be reached through a link.
fn resolved(folder: &Path) -> PathBuf {
    let Some(existing) = folder.ancestors().find(|above| above.exists()) else {
        return folder.to_path_buf();
    };
    let rest = folder.strip_prefix(existing).unwrap_or(Path::new(""));
    std::fs::canonicalize(existing)
        .unwrap_or_else(|_| existing.to_path_buf())
        .join(rest)
}

/// Where to watch for a skills folder: the folder itself once it exists, or
/// else the nearest folder above it, which shows it being made. Both are
/// watched to any depth: on macOS a shallow watch hears nothing from below.
fn watch_target(folder: &Path) -> PathBuf {
    let folder = resolved(folder);
    folder
        .ancestors()
        .find(|above| above.is_dir())
        .unwrap_or(&folder)
        .to_path_buf()
}

/// Whether a change at `path` can add a skill: it is inside a skills folder,
/// or on the way to one that is not made yet.
fn concerns(path: &Path, folders: &[PathBuf]) -> bool {
    folders
        .iter()
        .any(|folder| path.starts_with(folder) || folder.starts_with(path))
}

/// Watches each agent's global skills folder while the core runs, including
/// one that does not exist yet, and tells every client about a skill that
/// shows up in one, so the person can move it in.
pub(crate) fn watch_global(core: &Arc<Core>) {
    let core = Arc::downgrade(core);
    std::thread::spawn(move || {
        watch_folders(global_folders(), |skill| {
            let Some(core) = core.upgrade() else {
                return;
            };
            let mut kept = newly_added();
            if kept.len() >= MAX_ADDED {
                kept.remove(0);
            }
            kept.push(skill);
            drop(kept);
            core.changes.changed(Topic::Skills, None, None);
        });
    });
}

fn watch_folders(folders: Vec<(&'static str, PathBuf)>, mut tell: impl FnMut(GlobalSkillAdded)) {
    let mut known = HashSet::new();
    added_skills(&folders, &mut known);
    let real: Vec<PathBuf> = folders.iter().map(|(_, folder)| resolved(folder)).collect();
    let (changed, changes) = std::sync::mpsc::channel::<()>();
    let Ok(mut watcher) =
        notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if event.is_ok_and(|event| event.paths.iter().any(|path| concerns(path, &real))) {
                let _ = changed.send(());
            }
        })
    else {
        return;
    };
    let mut watching: HashSet<PathBuf> = HashSet::new();
    let mut follow = |watcher: &mut notify::RecommendedWatcher| {
        for (_, folder) in &folders {
            let target = watch_target(folder);
            if !watching.contains(&target)
                && watcher.watch(&target, RecursiveMode::Recursive).is_ok()
            {
                watching.insert(target);
            }
        }
    };
    follow(&mut watcher);
    while changes.recv().is_ok() {
        std::thread::sleep(Duration::from_millis(500));
        while changes.try_recv().is_ok() {}
        follow(&mut watcher);
        for added in added_skills(&folders, &mut known) {
            tell(added);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(folder: &Path, name: &str) {
        std::fs::create_dir_all(folder.join(name)).unwrap();
        std::fs::write(
            folder.join(name).join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {name} skill\n---\n"),
        )
        .unwrap();
    }

    #[test]
    fn the_overview_shows_both_sides_and_the_names_they_share() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let claude = root.path().join("claude-skills");
        let empty = root.path().join("empty-skills");
        std::fs::create_dir_all(&empty).unwrap();
        skills::create(&data, "deploy", "Deploys", "").unwrap();
        skills::create(&data, "review", "Reviews", "").unwrap();
        write_skill(&claude, "deploy");
        write_skill(&claude, "notes");

        let seen = library_overview(&data, vec![("Claude", claude.clone()), ("Pi", empty)]);
        assert_eq!(seen.skills.len(), 2);
        assert_eq!(seen.own.len(), 1);
        assert_eq!(seen.own[0].agent, "Claude");
        assert_eq!(seen.own[0].skills.len(), 2);
        assert_eq!(seen.clashes, vec!["deploy".to_string()]);
    }

    #[test]
    fn only_skills_that_were_not_there_before_count_as_added() {
        let root = tempfile::tempdir().unwrap();
        let claude = root.path().join("claude-skills");
        write_skill(&claude, "notes");
        let folders = vec![("Claude", claude.clone())];
        let mut known = HashSet::new();
        assert_eq!(added_skills(&folders, &mut known).len(), 1);
        assert!(added_skills(&folders, &mut known).is_empty());

        write_skill(&claude, "deploy");
        std::fs::create_dir_all(claude.join("half-written")).unwrap();
        let added = added_skills(&folders, &mut known);
        assert_eq!(
            added,
            vec![GlobalSkillAdded {
                agent: "Claude",
                name: "deploy".into(),
                path: claude.join("deploy"),
            }]
        );
    }

    #[test]
    fn a_skills_folder_not_made_yet_is_watched_from_the_folder_above() {
        let root = tempfile::tempdir().unwrap();
        let real = std::fs::canonicalize(root.path()).unwrap();
        let skills = root.path().join(".claude/skills");
        assert_eq!(resolved(&skills), real.join(".claude/skills"));
        assert_eq!(watch_target(&skills), real);
        std::fs::create_dir_all(&skills).unwrap();
        assert_eq!(watch_target(&skills), real.join(".claude/skills"));
    }

    #[test]
    fn only_changes_inside_or_on_the_way_to_a_skills_folder_count() {
        let folders = [PathBuf::from("/home/me/.claude/skills")];
        assert!(concerns(Path::new("/home/me/.claude"), &folders));
        assert!(concerns(
            Path::new("/home/me/.claude/skills/notes/SKILL.md"),
            &folders
        ));
        assert!(!concerns(
            Path::new("/home/me/projects/app/main.rs"),
            &folders
        ));
        assert!(!concerns(
            Path::new("/home/me/.claude/projects/log.jsonl"),
            &folders
        ));
    }

    #[test]
    fn a_skill_written_into_a_folder_made_after_watching_began_is_reported() {
        let root = tempfile::tempdir().unwrap();
        let skills = root.path().join(".claude/skills");
        let (told, heard) = std::sync::mpsc::channel();
        let folders = vec![("Claude", skills.clone())];
        std::thread::spawn(move || {
            watch_folders(folders, move |added| {
                let _ = told.send(added);
            })
        });
        std::thread::sleep(Duration::from_millis(300));

        std::fs::create_dir_all(&skills).unwrap();
        std::thread::sleep(Duration::from_millis(900));
        write_skill(&skills, "notes");
        let added = heard
            .recv_timeout(Duration::from_secs(10))
            .expect("the new skill is reported");
        assert_eq!(added.name, "notes");
        assert_eq!(added.agent, "Claude");
    }

    #[test]
    fn adopting_moves_a_global_skill_in_and_refuses_anything_else() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let claude = root.path().join("claude-skills");
        write_skill(&claude, "notes");
        let folders = vec![("Claude", claude.clone())];

        assert!(adopt_global(&data, &folders, &root.path().join("elsewhere/notes")).is_err());
        let skill = adopt_global(&data, &folders, &claude.join("notes")).unwrap();
        assert_eq!(skill.name, "notes");
        assert!(!claude.join("notes").exists());
        assert_eq!(skills::list(&data).len(), 1);
    }
}
