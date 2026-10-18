//! The Skills section of Settings: the Sikemux skills library, and the
//! person's own global skills beside it, which Sikemux never changes unless
//! asked to move one in.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use serde::Serialize;
use sikemux_core::skills::{self, Skill};
use tauri::{AppHandle, Emitter, Manager};

use crate::error::{AppError, AppResult};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnSkills {
    agent: &'static str,
    folder: PathBuf,
    skills: Vec<Skill>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsOverview {
    library: PathBuf,
    skills: Vec<Skill>,
    own: Vec<OwnSkills>,
    /// Names in the library that one of the person's own skills also uses.
    clashes: Vec<String>,
}

fn data_dir(app: &AppHandle) -> AppResult<PathBuf> {
    app.path()
        .app_data_dir()
        .map_err(|error| AppError::Other(format!("app data folder unavailable: {error}")))
}

fn overview(data_dir: &Path, folders: Vec<(&'static str, PathBuf)>) -> SkillsOverview {
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
fn adopt(
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

pub const GLOBAL_SKILL_ADDED_EVENT: &str = "global_skill_added";

/// A skill that appeared in an agent's own global folder while Sikemux ran,
/// most likely written there by that agent's own skill maker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalSkillAdded {
    agent: &'static str,
    name: String,
    path: PathBuf,
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

/// Watches each agent's global skills folder while the app runs, including
/// one that does not exist yet, and tells the window about a skill that shows
/// up in one, so it can offer to move it in.
pub fn watch_global(app: AppHandle) {
    std::thread::spawn(move || {
        watch_folders(crate::agents::global_skill_folders(), |added| {
            let _ = app.emit_to("main", GLOBAL_SKILL_ADDED_EVENT, added);
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

#[tauri::command]
pub async fn skills_overview(app: AppHandle) -> AppResult<SkillsOverview> {
    let data_dir = data_dir(&app)?;
    tokio::task::spawn_blocking(move || overview(&data_dir, crate::agents::global_skill_folders()))
        .await
        .map_err(|error| AppError::Other(error.to_string()))
}

#[tauri::command]
pub async fn skills_create(
    app: AppHandle,
    name: String,
    description: String,
    body: String,
) -> AppResult<Skill> {
    skills::create(&data_dir(&app)?, &name, &description, &body).map_err(AppError::Other)
}

#[tauri::command]
pub async fn skills_install(
    app: AppHandle,
    source: String,
    subfolder: Option<String>,
) -> AppResult<Skill> {
    let data_dir = data_dir(&app)?;
    tokio::task::spawn_blocking(move || {
        let source = source.trim();
        if source.starts_with('/') {
            skills::install_folder(&data_dir, Path::new(source))
        } else {
            let subfolder = subfolder
                .as_deref()
                .map(str::trim)
                .filter(|path| !path.is_empty());
            skills::install_git(&data_dir, source, subfolder)
        }
    })
    .await
    .map_err(|error| AppError::Other(error.to_string()))?
    .map_err(AppError::Other)
}

#[tauri::command]
pub async fn skills_remove(app: AppHandle, name: String) -> AppResult<()> {
    skills::remove(&data_dir(&app)?, &name).map_err(AppError::Other)
}

#[tauri::command]
pub async fn skills_adopt(app: AppHandle, path: String) -> AppResult<Skill> {
    let data_dir = data_dir(&app)?;
    tokio::task::spawn_blocking(move || {
        adopt(
            &data_dir,
            &crate::agents::global_skill_folders(),
            Path::new(&path),
        )
    })
    .await
    .map_err(|error| AppError::Other(error.to_string()))?
    .map_err(AppError::Other)
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

        let seen = overview(&data, vec![("Claude", claude.clone()), ("Pi", empty)]);
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

        assert!(adopt(&data, &folders, &root.path().join("elsewhere/notes")).is_err());
        let skill = adopt(&data, &folders, &claude.join("notes")).unwrap();
        assert_eq!(skill.name, "notes");
        assert!(!claude.join("notes").exists());
        assert_eq!(skills::list(&data).len(), 1);
    }
}
