//! The host's skills: the Sikemux library every agent here is handed, and
//! the person's own global skills beside it, which Sikemux never changes
//! unless asked to move one in. Any client manages them through these calls.

use crate::server::changed::ChangedCore;
use crate::server::changed::What;
use crate::server::serving::ServeParts;
use crate::server::watch_hub::{WatchHandle, WatchHub};
use crate::server::workspace_state::WorkspaceParts;
pub use sikemux_calls::skill_library::{
    GlobalSkillAdded, Located, Named, NewSkill, OwnSkills, SkillsOverview, Source,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::RecursiveMode;
use sikemux_process::sync::Locked;

use super::changed::Topic;
use crate::agent_catalog::AgentKind;
use crate::skills::{self, Skill};

use super::connection::blocking;
use super::{Core, CoreError, CoreResult};

/// The most global skills kept as newly added until the person answers.
const MAX_ADDED: usize = 64;

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
        let real = sikemux_process::files::real_path(folder).unwrap_or_else(|_| folder.clone());
        let fresh = !seen.contains(&real);
        seen.push(real);
        fresh
    });
    folders
}

/// The skills an agent saved in its own folder. A link into another folder
/// (agentctl fans ~/skills out this way) is the shared skill, not its own.
fn own_skills_in(folder: &Path) -> Vec<Skill> {
    let mut found = skills::list_in(folder);
    found.retain(|skill| {
        !std::fs::symlink_metadata(&skill.path).is_ok_and(|meta| meta.file_type().is_symlink())
    });
    found
}

fn library_overview(data_dir: &Path, folders: Vec<(&'static str, PathBuf)>) -> SkillsOverview {
    let library = skills::list(data_dir);
    let own: Vec<OwnSkills> = folders
        .into_iter()
        .map(|(agent, folder)| OwnSkills {
            agent: agent.to_owned(),
            skills: own_skills_in(&folder),
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

/// Runs `work` on the library in the data folder, off the async threads.
async fn on_library<T: Send + 'static>(
    core: &Core,
    work: impl FnOnce(&Path) -> CoreResult<T> + Send + 'static,
) -> CoreResult<T> {
    let data_dir = core
        .data_dir()
        .clone()
        .ok_or_else(|| CoreError::from("this host keeps no skills"))?;
    blocking(move || work(&data_dir)).await
}

pub async fn overview(core: &Core) -> CoreResult<SkillsOverview> {
    on_library(core, |data_dir| {
        Ok(library_overview(data_dir, global_folders()))
    })
    .await
}

pub async fn added(core: &Core) -> CoreResult<Vec<GlobalSkillAdded>> {
    on_library(core, |_| Ok(newly_added().clone())).await
}

pub async fn create(core: &Core, skill: NewSkill) -> CoreResult<Skill> {
    let created = on_library(core, move |data_dir| {
        Ok(skills::create(
            data_dir,
            &skill.name,
            &skill.description,
            &skill.body,
        )?)
    })
    .await?;
    core.changed(What::Read(Topic::Skills));
    Ok(created)
}

pub async fn install(core: &Core, from: Source) -> CoreResult<Skill> {
    let installed = on_library(core, move |data_dir| {
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
    .await?;
    core.changed(What::Read(Topic::Skills));
    Ok(installed)
}

pub async fn remove(core: &Core, named: Named) -> CoreResult<()> {
    on_library(core, move |data_dir| {
        Ok(skills::remove(data_dir, &named.name)?)
    })
    .await?;
    core.changed(What::Read(Topic::Skills));
    Ok(())
}

pub async fn adopt(core: &Core, located: Located) -> CoreResult<Skill> {
    let adopted = on_library(core, move |data_dir| {
        let skill = adopt_global(data_dir, &global_folders(), &located.path)?;
        newly_added().retain(|added| added.path != located.path);
        Ok(skill)
    })
    .await?;
    core.changed(What::Read(Topic::Skills));
    Ok(adopted)
}

fn added_skills(
    folders: &[(&'static str, PathBuf)],
    known: &mut HashSet<PathBuf>,
) -> Vec<GlobalSkillAdded> {
    let mut added = Vec::new();
    for (agent, folder) in folders {
        for skill in own_skills_in(folder) {
            if known.insert(skill.path.clone()) {
                added.push(GlobalSkillAdded {
                    agent: (*agent).to_owned(),
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
pub fn resolved(folder: &Path) -> PathBuf {
    let Some(existing) = folder.ancestors().find(|above| above.exists()) else {
        return folder.to_path_buf();
    };
    let rest = folder.strip_prefix(existing).unwrap_or(Path::new(""));
    sikemux_process::files::real_path(existing)
        .unwrap_or_else(|_| existing.to_path_buf())
        .join(rest)
}

/// Where to watch for a skills folder: the folder itself once it exists, or
/// else the nearest folder above it, which shows it being made. Both are
/// watched to any depth: on macOS a shallow watch hears nothing from below.
pub fn watch_target(folder: &Path) -> PathBuf {
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
pub fn watch_global(core: &Arc<Core>) {
    let hub = core.watch_hub().clone();
    let core = Arc::downgrade(core);
    tokio::spawn(async move {
        watch_folders(&hub, global_folders(), |skill| {
            let Some(core) = core.upgrade() else {
                return;
            };
            let mut kept = newly_added();
            if kept.len() >= MAX_ADDED {
                kept.remove(0);
            }
            kept.push(skill);
            drop(kept);
            core.changed(What::Read(Topic::Skills));
        })
        .await;
    });
}

async fn watch_folders(
    hub: &Arc<WatchHub>,
    folders: Vec<(&'static str, PathBuf)>,
    mut tell: impl FnMut(GlobalSkillAdded),
) {
    let mut known = HashSet::new();
    added_skills(&folders, &mut known);
    let real: Vec<PathBuf> = folders.iter().map(|(_, folder)| resolved(folder)).collect();
    let (changed, mut changes) = tokio::sync::mpsc::unbounded_channel::<()>();
    let watcher = hub.subscribe(move |event| {
        if event.paths.iter().any(|path| concerns(path, &real)) {
            let _ = changed.send(());
        }
    });
    let mut watching: HashSet<PathBuf> = HashSet::new();
    let mut follow = |watcher: &WatchHandle| {
        let targets: HashSet<PathBuf> = folders
            .iter()
            .map(|(_, folder)| watch_target(folder))
            .collect();
        if targets != watching {
            let watched = watcher.set_paths(
                targets
                    .into_iter()
                    .map(|target| (target, RecursiveMode::Recursive)),
            );
            watching = watched.into_keys().collect();
        }
    };
    follow(&watcher);
    while changes.recv().await.is_some() {
        tokio::time::sleep(Duration::from_millis(500)).await;
        while changes.try_recv().is_ok() {}
        follow(&watcher);
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

        let seen = library_overview(&data, vec![("Claude", claude), ("Pi", empty)]);
        assert_eq!(seen.skills.len(), 2);
        assert_eq!(seen.own.len(), 1);
        assert_eq!(seen.own[0].agent, "Claude");
        assert_eq!(seen.own[0].skills.len(), 2);
        assert_eq!(seen.clashes, vec!["deploy".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_skill_is_shared_not_added_or_own() {
        let root = tempfile::tempdir().unwrap();
        let shared = root.path().join("shared");
        let hermes = root.path().join("hermes-skills");
        write_skill(&shared, "linked");
        write_skill(&hermes, "real");
        std::os::unix::fs::symlink(shared.join("linked"), hermes.join("linked")).unwrap();
        let folders = vec![("Hermes", hermes.clone())];
        let added = added_skills(&folders, &mut HashSet::new());
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].name, "real");
        let seen = library_overview(&root.path().join("data"), folders);
        assert_eq!(seen.own[0].skills.len(), 1);
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
                agent: "Claude".to_owned(),
                name: "deploy".into(),
                path: claude.join("deploy"),
            }]
        );
    }

    #[test]
    fn a_skills_folder_not_made_yet_is_watched_from_the_folder_above() {
        let root = tempfile::tempdir().unwrap();
        let real = sikemux_process::files::real_path(root.path()).unwrap();
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

    #[tokio::test]
    async fn a_skill_written_into_a_folder_made_after_watching_began_is_reported() {
        let root = tempfile::tempdir().unwrap();
        let hub = Arc::new(WatchHub::new());
        let skills = root.path().join(".claude/skills");
        let (told, mut heard) = tokio::sync::mpsc::unbounded_channel();
        let folders = vec![("Claude", skills.clone())];
        let task = tokio::spawn({
            let hub = hub.clone();
            async move {
                watch_folders(&hub, folders, move |added| {
                    let _ = told.send(added);
                })
                .await;
            }
        });
        tokio::time::sleep(Duration::from_millis(300)).await;

        std::fs::create_dir_all(&skills).unwrap();
        let mut found = None;
        for attempt in 0..30 {
            write_skill(&skills, &format!("notes-{attempt}"));
            if let Ok(Some(added)) =
                tokio::time::timeout(Duration::from_secs(1), heard.recv()).await
            {
                found = Some(added);
                break;
            }
        }
        task.abort();
        let added = found.expect("a new skill is reported");
        assert!(added.name.starts_with("notes-"), "{}", added.name);
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
