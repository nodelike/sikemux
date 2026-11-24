//! The space a project takes from the projects above it. One with no space of its own is in
//! that of its nearest parent that is in one, where a folder the person keeps projects in holds
//! that parent and reaches the project within its depth.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sikemux_process::paths::expand_home;
use sikemux_view::prefs::ProjectRoot;
use sikemux_view::workspace::ProjectInfo;

use sikemux_process::sync::Locked;

struct Reach {
    root: PathBuf,
    depth: usize,
}

impl Reach {
    fn of(root: &ProjectRoot) -> Self {
        let typed = expand_home(&root.path);
        Self {
            root: sikemux_process::files::real_path(&typed).unwrap_or(typed),
            depth: root.depth as usize,
        }
    }

    fn links(&self, parent: &Path, child: &Path) -> bool {
        parent.starts_with(&self.root)
            && child
                .strip_prefix(&self.root)
                .is_ok_and(|below| below.components().count() <= self.depth)
    }
}

/// How far each folder the person keeps projects in reaches. The disk is asked where a folder
/// is only when the folders change, since the view is made many times a second.
#[derive(Default)]
pub struct Reaches(Mutex<(Vec<ProjectRoot>, Vec<Reach>)>);

impl Reaches {
    /// Fills in what each of `projects`, as kept, follows with `roots` as the person's folders.
    pub fn follow(&self, projects: &mut [ProjectInfo], roots: &[ProjectRoot]) {
        let mut known = self.0.locked();
        if known.0 != roots {
            *known = (roots.to_vec(), roots.iter().map(Reach::of).collect());
        }
        follow(projects, &known.1);
    }
}

fn follow(projects: &mut [ProjectInfo], reaches: &[Reach]) {
    let mut parents_first: Vec<&mut ProjectInfo> = projects.iter_mut().collect();
    parents_first.sort_by_key(|project| project.path.components().count());
    let mut placed: HashMap<PathBuf, (String, String)> = HashMap::new();
    for project in parents_first {
        let nearest = (project.path.ancestors().skip(1))
            .find_map(|folder| Some((folder, placed.get(folder)?)))
            .filter(|(folder, _)| (reaches.iter()).any(|reach| reach.links(folder, &project.path)))
            .map(|(_, parent)| parent.clone());
        project.space_followed = project.space.is_none() && nearest.is_some();
        if let Some((parent, space)) = nearest {
            project.space_parent = Some(parent);
            project.space.get_or_insert(space);
        } else {
            project.space_parent = None;
        }
        if let Some(space) = &project.space {
            placed.insert(project.path.clone(), (project.id.clone(), space.clone()));
        }
    }
}
