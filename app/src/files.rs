use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::base::Button as BaseButton;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_kit::component::{ThemeStyled, WindowExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, canvas, div, point, px, uniform_list, AnyElement, App, AppContext, Bounds, ClickEvent,
    ClipboardItem, Context, Entity, FontWeight, InteractiveElement, IntoElement, KeyBinding,
    KeyDownEvent, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, ParentElement, Pixels,
    PlatformInput, ScrollStrategy, SharedString, Styled, Subscription, UniformListScrollHandle,
    WeakEntity, Window,
};
use serde_json::{json, Value};
use sikemux_wire::protocol::PaneKind;

use crate::core::Target;
use crate::hosts::Place;
use crate::icons::Icon;
use crate::motion;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, TEXT_3XS, TEXT_XS,
};
use crate::toast;
use crate::ui::{self, IconButton};
use crate::workbench::{Strip, Workbench};

/// One thing in a folder, as the core listed it: folders first, then by name.
#[derive(Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub ignored: bool,
}

/// A row the tree draws: an entry at its depth under the project.
pub struct Row {
    pub path: PathBuf,
    pub depth: usize,
    pub entry: Entry,
    pub open: bool,
}

/// The project's folders as far as the person opened them. The listings are what the core
/// last answered for each; nothing here decides what a folder holds.
#[derive(Default)]
pub struct FileTree {
    pub root: Option<PathBuf>,
    place: Place,
    listed: HashMap<PathBuf, Vec<Entry>>,
    /// The pages of each listing on its way, shown only once the whole folder is in.
    incoming: HashMap<PathBuf, Vec<Entry>>,
    /// The folders being listed, and those that changed meanwhile and are listed once more after.
    asked: HashSet<PathBuf>,
    again: HashSet<PathBuf>,
    pub expanded: HashSet<PathBuf>,
    pub selected_dir: Option<PathBuf>,
    /// What git says of each changed file and the folders holding it, by absolute path.
    pub marks: HashMap<PathBuf, (Option<char>, Mark)>,
    /// The rows as last laid out, until a listing or an opened folder changes them.
    rows: RefCell<Option<Rc<Vec<Row>>>>,
    /// The name being typed for a new file or folder.
    pub naming: Option<Naming>,
    /// The row a right click last landed on, which the tree's one menu is about; none when
    /// it landed on the tree's empty space.
    menu_row: Option<(PathBuf, bool)>,
    /// The row the keys move from and act on while the tree has focus.
    keyed: Option<PathBuf>,
    /// Where the keyed row was last drawn, for its menu to open there.
    keyed_at: Rc<Cell<Option<Bounds<Pixels>>>>,
    scroll: UniformListScrollHandle,
    /// The file in front the tree last opened its folders to.
    revealed: Option<PathBuf>,
    /// Folders shown that changed since the tree was last drawn, listed again once it is, so
    /// a tree out of sight asks the core for nothing.
    behind: HashSet<PathBuf>,
}

actions!(
    file_tree,
    [Previous, Next, First, Last, Expand, Collapse, Activate, Rename, ShowMenu]
);

const CONTEXT: &str = "FileTree";

/// Binds the tree's keys, as the Mac's tree takes them.
pub fn install(cx: &mut App) {
    let context = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", Previous, context),
        KeyBinding::new("down", Next, context),
        KeyBinding::new("home", First, context),
        KeyBinding::new("end", Last, context),
        KeyBinding::new("right", Expand, context),
        KeyBinding::new("left", Collapse, context),
        KeyBinding::new("enter", Activate, context),
        KeyBinding::new("space", Activate, context),
        KeyBinding::new("f2", Rename, context),
        KeyBinding::new("shift-f10", ShowMenu, context),
    ]);
}

/// A new file or folder in `parent`, or `from` renamed, while the person types its name in the tree.
pub struct Naming {
    pub parent: PathBuf,
    pub dir: bool,
    pub from: Option<PathBuf>,
    pub input: Entity<InputState>,
    _typing: Subscription,
}

impl FileTree {
    /// Starts over on `root`, the project in front, unless it already shows it from that host.
    pub fn show(&mut self, root: Option<PathBuf>, core: &Target) {
        if root == self.root && core.place() == &self.place {
            return;
        }
        *self = FileTree {
            root: root.clone(),
            place: core.place().clone(),
            ..FileTree::default()
        };
        if let Some(root) = root {
            self.relist(vec![root], core);
        }
    }

    /// Lists every shown folder again from scratch, for when answers on their way were lost.
    pub fn relist_shown(&mut self, core: &Target) {
        self.asked.clear();
        self.again.clear();
        self.incoming.clear();
        let stale = self.stale(None);
        self.relist(stale, core);
    }

    /// Lists again the folders that changed while the tree was out of sight, once it is drawn.
    pub fn catch_up(&mut self, core: &Target) {
        let behind: Vec<PathBuf> = self.behind.drain().collect();
        if !behind.is_empty() {
            self.relist(behind, core);
        }
    }

    /// Asks the core to list `dirs`; one already being listed is listed once more when it is in.
    fn relist(&mut self, dirs: Vec<PathBuf>, core: &Target) {
        let fresh: Vec<PathBuf> = dirs
            .into_iter()
            .filter(|dir| {
                if self.asked.contains(dir) {
                    self.again.insert(dir.clone());
                    return false;
                }
                self.asked.insert(dir.clone());
                true
            })
            .collect();
        list(core, &fresh, None);
    }

    /// Takes in a `files.list` answer, asking for the next page of any folder it left short.
    pub fn listed(&mut self, answer: &Value, core: &Target) {
        for folder in answer.as_array().into_iter().flatten() {
            let Some(dir) = folder["dir"].as_str().map(PathBuf::from) else {
                continue;
            };
            if folder["error"].is_string() {
                self.asked.remove(&dir);
                self.again.remove(&dir);
                self.incoming.remove(&dir);
                continue;
            }
            let entries: Vec<Entry> = folder["entries"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|entry| {
                    Some(Entry {
                        name: entry["name"].as_str()?.to_owned(),
                        dir: entry["dir"].as_bool().unwrap_or(false),
                        ignored: entry["ignored"].as_bool().unwrap_or(false),
                    })
                })
                .collect();
            let total = folder["total"].as_u64().unwrap_or(0) as usize;
            let got = entries.len();
            let incoming = self.incoming.entry(dir.clone()).or_default();
            incoming.extend(entries);
            if got > 0 && incoming.len() < total {
                list_from(core, &dir, incoming.len());
                continue;
            }
            let whole = self.incoming.remove(&dir).unwrap_or_default();
            if self.listed.get(&dir) != Some(&whole) {
                self.listed.insert(dir.clone(), whole);
                self.rows.take();
            }
            self.asked.remove(&dir);
            if self.again.remove(&dir) {
                self.relist(vec![dir], core);
            }
        }
    }

    /// Notes the folders shown that `paths` (relative to the project) could have changed, or
    /// every shown folder when the core did not say what moved, to list again once the tree
    /// is drawn; answers whether the tree has to be drawn for that.
    pub fn changed(&mut self, paths: Option<&[String]>) -> bool {
        let was_behind = !self.behind.is_empty();
        let stale = self.stale(paths);
        self.behind.extend(stale);
        !was_behind && !self.behind.is_empty()
    }

    fn stale(&self, paths: Option<&[String]>) -> Vec<PathBuf> {
        let Some(root) = self.root.clone() else {
            return Vec::new();
        };
        let shown: Vec<PathBuf> = std::iter::once(root.clone())
            .chain(self.expanded.iter().cloned())
            .collect();
        match paths.filter(|paths| !paths.is_empty() && !paths.iter().any(|path| path == ".git")) {
            None => shown,
            Some(paths) => {
                let moved: Vec<PathBuf> = paths.iter().map(|path| root.join(path)).collect();
                shown
                    .into_iter()
                    .filter(|dir| {
                        moved.iter().any(|path| {
                            path.parent() == Some(dir.as_path())
                                || dir.starts_with(path)
                                || (path.file_name().is_some_and(|name| name == ".gitignore")
                                    && path.parent().is_some_and(|folder| dir.starts_with(folder)))
                        })
                    })
                    .collect()
            }
        }
    }

    pub fn toggle(&mut self, dir: &Path, core: &Target) {
        self.selected_dir = Some(dir.to_owned());
        self.rows.take();
        if !self.expanded.remove(dir) {
            self.expanded.insert(dir.to_owned());
            if !self.listed.contains_key(dir) {
                self.relist(vec![dir.to_owned()], core);
            }
        }
    }

    pub fn is_dir(&self, path: &Path) -> bool {
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return false;
        };
        self.listed
            .get(parent)
            .into_iter()
            .flatten()
            .any(|entry| entry.dir && entry.name == name.to_string_lossy())
    }

    /// Lists `dir` again, as after something was made in it.
    pub fn reload(&mut self, dir: &Path, core: &Target) {
        self.relist(vec![dir.to_owned()], core);
    }

    /// Opens the folders down to `file` once it comes to the front.
    pub fn reveal(&mut self, file: Option<PathBuf>, core: &Target) {
        if file == self.revealed {
            return;
        }
        if let Some(folder) = file.as_deref().and_then(Path::parent) {
            self.open_down_to(folder, core);
        }
        self.revealed = file;
    }

    /// Opens the folders down to `dir`, from the project's folder, listing any not yet listed.
    pub fn open_down_to(&mut self, dir: &Path, core: &Target) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let Ok(rest) = dir.strip_prefix(&root) else {
            return;
        };
        let mut folder = root;
        let mut unlisted = Vec::new();
        for part in rest.components() {
            folder.push(part);
            if self.expanded.insert(folder.clone()) {
                self.rows.take();
            }
            if !self.listed.contains_key(&folder) {
                unlisted.push(folder.clone());
            }
        }
        if !unlisted.is_empty() {
            self.relist(unlisted, core);
        }
    }

    /// Where the row typing a name goes, how deep, and whether it stands in for the row of
    /// what is renamed rather than coming after its folder's.
    fn naming_at(&self, rows: &[Row]) -> Option<(usize, usize, bool)> {
        let naming = self.naming.as_ref()?;
        if let Some(from) = &naming.from {
            let at = rows.iter().position(|row| &row.path == from)?;
            return Some((at, rows[at].depth, true));
        }
        Some(
            match rows.iter().position(|row| row.path == naming.parent) {
                Some(at) => (at + 1, rows[at].depth + 1, false),
                None => (0, 0, false),
            },
        )
    }

    pub fn collapse_all(&mut self) {
        self.expanded.clear();
        self.rows.take();
    }

    /// The row the keys are on and what is being renamed, for the test bundle's driver to check.
    #[cfg(feature = "drive")]
    pub fn drive_state(&self) -> Value {
        json!({
            "keyed": self.keyed,
            "renaming": self.naming.as_ref().and_then(|naming| naming.from.clone()),
        })
    }

    /// Git's letter for the changed file at `path`, drawn in its mark's colour.
    pub fn letter(&self, path: &Path, cx: &App) -> Option<gpui_kit::Div> {
        let (Some(letter), mark) = *self.marks.get(path)? else {
            return None;
        };
        Some(
            ui::text(letter.to_string(), TEXT_3XS, FontWeight::BOLD)
                .opacity(0.8)
                .text_color(mark.colour(Tokens::get(cx))),
        )
    }

    /// Takes in what git says of the project's files.
    pub fn mark(&mut self, overview: &Value) {
        if let Some(root) = &self.root {
            self.marks = git_marks(root, overview);
        }
    }

    /// The rows to draw: the project's entries, and those of every open folder under its own.
    pub fn rows(&self) -> Rc<Vec<Row>> {
        self.rows
            .borrow_mut()
            .get_or_insert_with(|| {
                let mut rows = Vec::new();
                if let Some(root) = &self.root {
                    self.walk(root, 0, &mut rows);
                }
                Rc::new(rows)
            })
            .clone()
    }

    fn walk(&self, dir: &Path, depth: usize, rows: &mut Vec<Row>) {
        for entry in self.listed.get(dir).into_iter().flatten() {
            let path = dir.join(&entry.name);
            let open = entry.dir && self.expanded.contains(&path);
            rows.push(Row {
                path: path.clone(),
                depth,
                entry: entry.clone(),
                open,
            });
            if open {
                self.walk(&path, depth + 1, rows);
            }
        }
    }
}

/// How git sees a file, as one letter and the colour class it is drawn with.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mark {
    Modified,
    Renamed,
    Added,
    Deleted,
    Untracked,
    Conflicted,
}

impl Mark {
    fn of(code: &str) -> Option<(char, Mark)> {
        Some(match code {
            "?" => ('U', Mark::Untracked),
            "U" => ('U', Mark::Conflicted),
            "A" => ('A', Mark::Added),
            "D" => ('D', Mark::Deleted),
            "R" => ('R', Mark::Renamed),
            "C" => ('C', Mark::Renamed),
            "T" => ('T', Mark::Modified),
            "M" => ('M', Mark::Modified),
            " " | "" => return None,
            _ => (code.chars().next()?, Mark::Modified),
        })
    }

    pub fn colour(self, t: &Tokens) -> gpui_kit::Hsla {
        match self {
            Mark::Modified => t.git_modified,
            Mark::Untracked => t.git_untracked,
            Mark::Added => t.git_added,
            Mark::Deleted => t.git_deleted,
            Mark::Renamed => t.git_renamed,
            Mark::Conflicted => t.danger,
        }
    }
}

/// Every changed file's letter and mark, and every folder holding changes with the most
/// telling mark below it, by absolute path.
fn git_marks(root: &Path, overview: &Value) -> HashMap<PathBuf, (Option<char>, Mark)> {
    let mut marks: HashMap<PathBuf, (Option<char>, Mark)> = HashMap::new();
    for file in overview["status"]["files"].as_array().into_iter().flatten() {
        let Some(path) = file["path"].as_str() else {
            continue;
        };
        let sides = [
            file["index"].as_str().unwrap_or(" "),
            file["worktree"].as_str().unwrap_or(" "),
        ];
        let Some((letter, mark)) = sides
            .into_iter()
            .filter_map(Mark::of)
            .max_by_key(|(_, mark)| *mark)
        else {
            continue;
        };
        let full = root.join(path.trim_end_matches('/'));
        let mut folder = full.parent();
        marks.insert(full.clone(), (Some(letter), mark));
        while let Some(dir) = folder.filter(|dir| dir.starts_with(root) && *dir != root) {
            let held = marks.entry(dir.to_owned()).or_insert((None, mark));
            if held.0.is_none() && mark > held.1 {
                held.1 = mark;
            }
            folder = dir.parent();
        }
    }
    marks
}

fn list(core: &Target, dirs: &[PathBuf], from: Option<usize>) {
    let dirs: Vec<String> = dirs
        .iter()
        .map(|dir| dir.to_string_lossy().into_owned())
        .collect();
    for batch in dirs.chunks(512) {
        let params = match from {
            Some(from) => json!({ "dirs": batch, "from": from }),
            None => json!({ "dirs": batch }),
        };
        core.answer("files.list", "files.list", params);
    }
}

fn list_from(core: &Target, dir: &Path, from: usize) {
    list(core, &[dir.to_owned()], Some(from));
}

/// The tab rail's Files page: the project's name and what can be made in it, then its tree.
pub fn page(bench: &Workbench, cx: &mut Context<Workbench>) -> impl IntoElement {
    if !bench.files.behind.is_empty() {
        let bench = cx.entity();
        cx.defer(move |cx| {
            bench.update(cx, |bench, _| {
                let core = bench.target();
                bench.files.catch_up(&core);
            })
        });
    }
    let t = Tokens::get(cx).clone();
    let name = bench
        .files
        .root
        .as_ref()
        .and_then(|root| root.file_name())
        .map_or_else(
            || "files".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
    let rows = bench.files.rows();
    let count =
        rows.len() + usize::from(matches!(bench.files.naming_at(&rows), Some((_, _, false))));
    let action = |id: &'static str, glyph: Icon, label: &'static str| {
        IconButton::new(id, glyph, label).small()
    };
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(
            div()
                .flex_none()
                .min_h(px(34.))
                .pt(SPACE_1H)
                .pr(SPACE_1H)
                .pb(SPACE_1H)
                .pl(SPACE_2H)
                .flex()
                .items_center()
                .justify_between()
                .gap(SPACE_2)
                .child(
                    ui::text(name.to_uppercase(), TEXT_3XS, FontWeight::SEMIBOLD)
                        .letter_spacing(0.08)
                        .text_color(t.text_tertiary),
                )
                .child(
                    div()
                        .flex()
                        .gap(SPACE_0)
                        .opacity(0.6)
                        .child(action("files-find", Icon::Search, "Find a file").on_click(
                            cx.listener(|bench, _, window, cx| {
                                bench.toggle_finder(window, cx);
                                cx.notify();
                            }),
                        ))
                        .child(action("files-new", Icon::FilePlus, "New file").on_click(
                            cx.listener(|bench, _, window, cx| {
                                bench.name_new_entry(false, window, cx)
                            }),
                        ))
                        .child(
                            action("files-new-folder", Icon::FolderPlus, "New folder").on_click(
                                cx.listener(|bench, _, window, cx| {
                                    bench.name_new_entry(true, window, cx)
                                }),
                            ),
                        )
                        .child(
                            action("files-collapse", Icon::CollapseAll, "Collapse folders")
                                .on_click(cx.listener(|bench, _, _, cx| {
                                    bench.files.collapse_all();
                                    cx.notify();
                                })),
                        ),
                ),
        )
        .children(open_files(bench, cx))
        .child({
            let rows = uniform_list(
                "file-rows",
                count,
                cx.processor(|bench: &mut Workbench, range: Range<usize>, window, cx| {
                    let front = bench.front_file().or_else(|| bench.desk_file(cx));
                    let rows = bench.files.rows();
                    let naming = bench.files.naming_at(&rows);
                    range
                        .filter_map(|index| {
                            let row = match naming {
                                Some((at, depth, _)) if index == at => {
                                    let naming = bench.files.naming.as_ref()?;
                                    return Some(naming_row(naming, depth, cx));
                                }
                                Some((at, _, false)) if index > at => rows.get(index - 1)?,
                                _ => rows.get(index)?,
                            };
                            Some(row_view(row, bench, front.as_deref(), window, cx))
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&bench.files.scroll)
            .flex_1()
            .min_h_0()
            .px(SPACE_1H)
            .pb(SPACE_1);
            macro_rules! key {
                ($action:ident) => {
                    cx.listener(|bench, _: &$action, window, cx| {
                        on_key(bench, Key::$action, window, cx)
                    })
                };
            }
            let tree = div()
                .id("file-tree")
                .key_context(CONTEXT)
                .track_focus(&bench.files_focus)
                .on_action(key!(Previous))
                .on_action(key!(Next))
                .on_action(key!(First))
                .on_action(key!(Last))
                .on_action(key!(Expand))
                .on_action(key!(Collapse))
                .on_action(key!(Activate))
                .on_action(key!(Rename))
                .on_action(key!(ShowMenu))
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col();
            match bench.files.root.clone() {
                Some(root) => {
                    let (this, here) = (cx.weak_entity(), matches!(bench.files.place, Place::This));
                    crate::drag::takes_drops(tree, root.clone(), this.clone())
                        .capture_any_mouse_down(cx.listener(|bench, _: &MouseDownEvent, _, _| {
                            bench.files.menu_row = None
                        }))
                        .context_menu(ui::menu(move |popup, _, cx| {
                            let row = this
                                .upgrade()
                                .and_then(|bench| bench.read(cx).files.menu_row.clone());
                            match row {
                                Some((path, dir)) => {
                                    row_menu(popup, &this, &path, dir, Some(&root), here, cx)
                                }
                                None => row_menu(popup, &this, &root, true, Some(&root), here, cx),
                            }
                        }))
                        .child(rows)
                        .into_any_element()
                }
                None => tree.child(rows).into_any_element(),
            }
        })
}

/// A file's icon by its name, in the box rows line marks up in.
pub(crate) fn file_glyph(name: &str, cx: &mut Context<Workbench>) -> AnyElement {
    ui::mark_box(22., crate::file_icons::glyph(name, 16., cx)).into_any_element()
}

/// The files open in the project's tabs, listed above its tree as the Mac's Files page does.
fn open_files(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let strip = bench
        .strip()
        .filter(|strip| matches!(strip, Strip::Project(_)))?;
    let open: Vec<(String, PathBuf, _)> = bench
        .tabs(&strip)
        .iter()
        .filter_map(|tab| match crate::tabs::first_view(&tab.pane)? {
            (PaneKind::Files, target) => Some((
                tab.id.clone(),
                PathBuf::from(target),
                crate::tabs::tab_menu(bench, &strip, tab, cx),
            )),
            _ => None,
        })
        .collect();
    if open.is_empty() {
        return None;
    }
    let t = Tokens::get(cx).clone();
    let front = bench.front_file();
    let root = bench.files.root.clone();
    let rows = open.into_iter().map(|(tab, path, menu)| {
        let active = front.as_deref() == Some(path.as_path());
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let folder = path
            .parent()
            .zip(root.as_deref())
            .and_then(|(dir, root)| dir.strip_prefix(root).ok())
            .map(|dir| dir.display().to_string())
            .filter(|dir| !dir.is_empty());
        let strip = strip.clone();
        let menu_id = SharedString::from(format!("open-file-menu-{tab}"));
        let row = BaseButton::new(SharedString::from(format!("open-file-{tab}")))
            .accessibility_label(SharedString::from(name.clone()))
            .w_full()
            .h(px(ROW_HEIGHT))
            .pl(SPACE_1)
            .pr(SPACE_2)
            .flex()
            .items_center()
            .justify_start()
            .gap(px(5.))
            .rounded(theme::RADIUS_2)
            .text_size(crate::theme::ui_text(TEXT_XS))
            .font_weight(FontWeight::MEDIUM)
            .text_color(if active {
                t.text_primary
            } else {
                t.text_secondary
            })
            .when(active, |row| row.bg(t.surface_chosen))
            .when(!active, |row| {
                row.hover(move |style| style.bg(t.surface_hover).text_color(t.text_primary))
            })
            .on_click(cx.listener(move |bench, _, _, _| bench.activate_tab(&strip, &tab)))
            .child(file_glyph(&name, cx))
            .child(div().min_w_0().truncate().child(name))
            .children(folder.map(|folder| {
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(t.text_tertiary)
                    .child(folder)
            }));
        div()
            .id(menu_id)
            .context_menu(ui::menu(menu))
            .child(row)
            .into_any_element()
    });
    let rows: Vec<AnyElement> = rows.collect();
    Some(
        div()
            .flex_none()
            .mx(SPACE_1H)
            .mb(SPACE_1)
            .pb(SPACE_1)
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(t.border)
            .children(rows)
            .into_any_element(),
    )
}

fn row_view(
    row: &Row,
    bench: &Workbench,
    front: Option<&Path>,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let tree = &bench.files;
    let marks = &tree.marks;
    let t = Tokens::get(cx).clone();
    let mark = marks.get(&row.path).copied();
    let colour = |mark: Mark| mark.colour(&t);
    let active = !row.entry.dir && front == Some(row.path.as_path());
    let ink = match mark {
        _ if active => t.text_primary,
        Some((_, mark)) => colour(mark),
        None if row.entry.ignored => t.text_disabled,
        None => t.text_secondary,
    };
    let pad = 4. + row.depth as f32 * 13.;
    let path = row.path.clone();
    let keyed = tree.keyed.as_ref() == Some(&row.path)
        && bench.files_focus.is_focused(window)
        && window.last_input_was_keyboard();
    let lead = if row.entry.dir {
        div()
            .flex()
            .items_center()
            .gap(px(5.))
            .child(motion::turn(
                format!("tree-chevron:{}", row.path.display()),
                motion::fold_angle(row.open),
                motion::TREE_FOLD,
                ui::icon(Icon::Chevron, 11.).text_color(t.text_disabled),
            ))
            .child(ui::icon(Icon::Folder, 17.).text_color(t.text_tertiary))
            .into_any_element()
    } else {
        file_glyph(&row.entry.name, cx)
    };
    let tail = match mark {
        Some((Some(_), _)) if !row.entry.dir => tree
            .letter(&row.path, cx)
            .map(|letter| letter.ml_auto().pl(SPACE_2).into_any_element()),
        Some((_, mark)) if row.entry.dir => Some(
            ui::text("●", 7., FontWeight::NORMAL)
                .ml_auto()
                .pl(SPACE_2)
                .text_color(colour(mark))
                .into_any_element(),
        ),
        _ => None,
    };
    let drawn = BaseButton::new(SharedString::from(format!("file-{}", row.path.display())))
        .accessibility_label(SharedString::from(row.entry.name.clone()))
        .w_full()
        .h(px(ROW_HEIGHT))
        .pl(px(if row.entry.dir { pad } else { pad + 13. }))
        .pr(SPACE_2)
        .flex()
        .items_center()
        .justify_start()
        .gap(px(5.))
        .rounded(theme::RADIUS_2)
        .text_size(crate::theme::ui_text(TEXT_XS))
        .font_weight(FontWeight::MEDIUM)
        .text_color(ink)
        .when(active, |row| row.bg(t.surface_chosen))
        .when(!active, |row| {
            row.hover(move |style| style.bg(t.surface_hover).text_color(t.text_primary))
        })
        .when(keyed, |row| row.focus_ring_style(window, cx))
        .on_click(cx.listener(move |bench, event: &ClickEvent, window, cx| {
            window.focus(&bench.files_focus, cx);
            bench.files.keyed = Some(path.clone());
            click_row(bench, &path, event.click_count(), cx);
        }))
        .child(lead)
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(row.entry.name.clone()),
        )
        .children(tail)
        .into_any_element();
    let pressed = (row.path.clone(), row.entry.dir);
    let drawn = div()
        .children(ui::spot(|| format!("tree-row-{}", row.entry.name)))
        .when(tree.keyed.as_ref() == Some(&row.path), |drawn| {
            let at = tree.keyed_at.clone();
            drawn.relative().child(
                canvas(move |bounds, _, _| at.set(Some(bounds)), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
        })
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |bench, _: &MouseDownEvent, _, _| {
                bench.files.menu_row = Some(pressed.clone())
            }),
        )
        .child(drawn)
        .into_any_element();
    crate::drag::tree_row(drawn, &row.path, row.entry.dir, &cx.entity(), cx)
}

const ROW_HEIGHT: f32 = 23.;

/// What clicking a row does: a folder opens or closes, a file shows in the editor, and stays
/// there once double-clicked.
fn click_row(bench: &mut Workbench, path: &Path, clicks: usize, cx: &mut Context<Workbench>) {
    if bench.files.expanded.contains(path) || bench.files.is_dir(path) {
        if clicks == 1 {
            bench.files.toggle(path, &bench.target());
        }
    } else if clicks == 2 {
        bench.open_file(path, cx);
    } else {
        bench.preview_file(path);
    }
    cx.notify();
}

#[derive(Clone, Copy)]
enum Key {
    Previous,
    Next,
    First,
    Last,
    Expand,
    Collapse,
    Activate,
    Rename,
    ShowMenu,
}

/// A key on the tree, acting on the row the keys are on: the one they last moved to while it is
/// still shown, else the first.
fn on_key(bench: &mut Workbench, key: Key, window: &mut Window, cx: &mut Context<Workbench>) {
    let rows = bench.files.rows();
    let keyed = bench
        .files
        .keyed
        .as_ref()
        .and_then(|keyed| rows.iter().position(|row| &row.path == keyed));
    let Some(at) = keyed.or((!rows.is_empty()).then_some(0)) else {
        return;
    };
    let (row, last) = (&rows[at], rows.len() - 1);
    let to = match key {
        Key::Previous => Some(at.saturating_sub(1)),
        Key::Next => Some((at + 1).min(last)),
        Key::First => Some(0),
        Key::Last => Some(last),
        Key::Expand if row.entry.dir && !row.open => {
            bench.files.toggle(&row.path, &bench.target());
            Some(at)
        }
        Key::Expand if row.open => Some((at + 1).min(last)),
        Key::Collapse if row.open => {
            bench.files.toggle(&row.path, &bench.target());
            Some(at)
        }
        Key::Collapse => rows[..at].iter().rposition(|above| above.depth < row.depth),
        Key::Expand => None,
        Key::Activate => {
            click_row(bench, &row.path.clone(), 1, cx);
            Some(at)
        }
        Key::Rename => {
            bench.rename_entry(&row.path.clone(), row.entry.dir, window, cx);
            None
        }
        Key::ShowMenu => {
            open_menu_at(bench.files.keyed_at.get(), window, cx);
            None
        }
    };
    if let Some(to) = to {
        bench.files.keyed = Some(rows[to].path.clone());
        bench
            .files
            .scroll
            .scroll_to_item(to, ScrollStrategy::Nearest);
    }
    cx.notify();
}

/// Opens the tree's menu over the row drawn at `bounds`, at its bottom-left as the Mac does, by
/// right-clicking there once the key has been handled: Kit's menus open only from a click.
fn open_menu_at(bounds: Option<Bounds<Pixels>>, window: &mut Window, cx: &mut App) {
    let Some(bounds) = bounds else {
        return;
    };
    let position = point(bounds.left() + px(1.), bounds.bottom() - px(1.));
    window.defer(cx, move |window, cx| {
        let modifiers = Modifiers::default();
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Right,
                position,
                modifiers,
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Right,
                position,
                modifiers,
                click_count: 1,
            }),
            cx,
        );
    });
}

/// The row a new file's or folder's name is typed in, where it will appear.
fn naming_row(naming: &Naming, depth: usize, cx: &mut Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    div()
        .w_full()
        .h(px(ROW_HEIGHT))
        .pl(px(4.
            + depth as f32 * 13.
            + if naming.dir { 0. } else { 13. }))
        .pr(SPACE_2)
        .flex()
        .items_center()
        .gap(px(5.))
        .capture_key_down(cx.listener(|bench, event: &KeyDownEvent, _, cx| {
            if event.keystroke.key == "escape" {
                bench.files.naming = None;
                cx.stop_propagation();
                cx.notify();
            }
        }))
        .child(
            ui::icon(if naming.dir { Icon::Folder } else { Icon::File }, 16.)
                .text_color(t.text_tertiary),
        )
        .child(
            Input::new(&naming.input)
                .appearance(false)
                .p_0()
                .text_size(crate::theme::ui_text(TEXT_XS))
                .text_color(t.text_primary),
        )
        .into_any_element()
}

/// Whether `name` can name a file or folder: something, with no path in it.
fn valid_entry_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

impl Workbench {
    /// Asks in the tree for the name of a new file, or folder, in the folder chosen there,
    /// else the file in front's, else the project's.
    pub fn name_new_entry(&mut self, dir: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.files.root.clone() else {
            return;
        };
        let parent = self
            .files
            .selected_dir
            .clone()
            .or_else(|| {
                self.front_file()
                    .and_then(|file| file.parent().map(Path::to_path_buf))
            })
            .filter(|parent| parent.starts_with(&root))
            .unwrap_or(root.clone());
        if parent != root && !self.files.expanded.contains(&parent) {
            self.files.toggle(&parent, &self.target());
        }
        self.ask_name(parent, dir, None, window, cx);
    }

    /// Asks in the tree, in `path`'s own row, for the name to give it, with its name up to its
    /// extension chosen.
    pub fn rename_entry(
        &mut self,
        path: &Path,
        dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(parent) = path.parent() else {
            return;
        };
        self.ask_name(parent.to_owned(), dir, Some(path.to_owned()), window, cx);
    }

    fn ask_name(
        &mut self,
        parent: PathBuf,
        dir: bool,
        from: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = from
            .as_ref()
            .and_then(|from| from.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stem = match name.rfind('.') {
            Some(dot) if dot > 0 && !dir => dot,
            _ => name.len(),
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(if dir { "folder name" } else { "file name" })
                .default_value(name.clone())
        });
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.set_selected_range(0..stem, cx);
        });
        let typing = cx.subscribe_in(
            &input,
            window,
            |bench, input, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    let name = input.read(cx).value().trim().to_owned();
                    bench.make_entry(&name, window, cx);
                }
                InputEvent::Blur => {
                    bench.files.naming = None;
                    cx.notify();
                }
                _ => {}
            },
        );
        self.files.naming = Some(Naming {
            parent,
            dir,
            from,
            input,
            _typing: typing,
        });
        cx.notify();
    }

    /// Makes the file or folder named in the tree, opening a new file to write in.
    fn make_entry(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !valid_entry_name(name) {
            return;
        }
        let Some(naming) = self.files.naming.take() else {
            return;
        };
        let path = naming.parent.join(name);
        let made = match &naming.from {
            Some(from) if *from == path => return cx.notify(),
            Some(from) => self
                .target()
                .request("files.rename", json!({ "from": from, "to": path })),
            None => self
                .target()
                .request("files.create", json!({ "path": path, "dir": naming.dir })),
        };
        let (parent, dir, from) = (
            naming.parent,
            naming.dir || naming.from.is_some(),
            naming.from,
        );
        cx.spawn_in(window, async move |bench, cx| {
            let made = made.await;
            bench
                .update(cx, |bench, cx| {
                    match made {
                        Ok(_) => {
                            bench.files.reload(&parent, &bench.target());
                            match &from {
                                Some(from) => bench.follow_rename(from, &path, cx),
                                None if !dir => bench.open_file(&path, cx),
                                None => {}
                            }
                        }
                        Err(error) => bench.notify(toast::error(error), cx),
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
        cx.notify();
    }

    /// Moves the editors and tabs of `from`, or of the files under it, to the same names under
    /// `to`, each tab in its own place in the strip, leaving the tab in front where it was.
    fn follow_rename(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        crate::editor::renamed(self, &self.place(), from, to, cx);
        self.rename_desk_files(from, to, cx);
        let Some(strip) = self.strip() else {
            return;
        };
        let front = self.tab().map(|tab| tab.id.clone());
        let moved: Vec<(usize, String, PathBuf)> = self
            .tabs(&strip)
            .iter()
            .enumerate()
            .filter_map(|(at, tab)| {
                let (kind, target) = crate::tabs::first_view(&tab.pane)?;
                let rest = Path::new(target).strip_prefix(from).ok()?;
                (kind == PaneKind::Files).then(|| (at, tab.id.clone(), to.join(rest)))
            })
            .collect();
        if moved.is_empty() {
            return;
        }
        let core = self.target();
        cx.spawn(async move |_, _| {
            for (at, old, path) in moved {
                let opened = core
                    .request(
                        "openTab",
                        strip.with(json!({ "kind": "files", "target": path.to_string_lossy() })),
                    )
                    .await;
                let Ok(Value::String(tab)) = opened else {
                    continue;
                };
                let _ = core
                    .request("moveTab", strip.with(json!({ "tab": tab, "to": at })))
                    .await;
                let _ = core
                    .request("closeTab", strip.with(json!({ "tab": old })))
                    .await;
            }
            if let Some(front) = front {
                let _ = core
                    .request("activateTab", strip.with(json!({ "tab": front })))
                    .await;
            }
        })
        .detach();
    }

    /// Moves `path` to the Trash once the person confirms.
    pub fn delete_entry(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let name = path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        self.confirm(
            format!("Move \"{name}\" to the Trash?"),
            path.to_string_lossy().into_owned(),
            "Move to Trash",
            true,
            move |bench, yes, _, cx| {
                if !yes {
                    return;
                }
                let deleted = bench
                    .target()
                    .request("files.delete", json!({ "path": path }));
                let parent = path.parent().map(Path::to_path_buf);
                cx.spawn(async move |bench, cx| {
                    let deleted = deleted.await;
                    bench
                        .update(cx, |bench, cx| {
                            match deleted {
                                Ok(_) => {
                                    if let Some(parent) = &parent {
                                        bench.files.reload(parent, &bench.target());
                                    }
                                }
                                Err(error) => bench.notify(toast::error(error), cx),
                            }
                            cx.notify();
                        })
                        .ok();
                })
                .detach();
            },
            cx,
        );
    }

    /// Moves `from` into the folder `into` on the host in front, taking its open editors and
    /// tabs along.
    pub fn move_entry(&mut self, from: PathBuf, into: PathBuf, cx: &mut Context<Self>) {
        let Some(name) = from.file_name() else {
            return;
        };
        let to = into.join(name);
        let moved = (self.target()).request("files.rename", json!({ "from": from, "to": to }));
        cx.spawn(async move |bench, cx| {
            let moved = moved.await;
            bench
                .update(cx, |bench, cx| {
                    match moved {
                        Ok(_) => {
                            let core = bench.target();
                            if let Some(parent) = from.parent() {
                                bench.files.reload(parent, &core);
                            }
                            bench.files.reload(&into, &core);
                            bench.files.open_down_to(&into, &core);
                            bench.follow_rename(&from, &to, cx);
                        }
                        Err(error) => bench.notify(toast::error(error), cx),
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }

    /// Copies files from this disk into the folder `into`; another host's core cannot reach
    /// them, so a drop there is refused.
    pub fn copy_in(&mut self, from: Vec<PathBuf>, into: PathBuf, cx: &mut Context<Self>) {
        if self.place() != crate::hosts::Place::This {
            return self.notify(
                toast::error(
                    "Files from this device can't be dropped into another host's project.",
                ),
                cx,
            );
        }
        let core = self.target();
        let copies: Vec<_> = (from.iter())
            .map(|from| core.request("files.copyInto", json!({ "from": from, "dir": into })))
            .collect();
        cx.spawn(async move |bench, cx| {
            let mut failed = None;
            for copied in copies {
                if let Err(error) = copied.await {
                    failed = Some(error);
                }
            }
            bench
                .update(cx, |bench, cx| {
                    let core = bench.target();
                    bench.files.reload(&into, &core);
                    bench.files.open_down_to(&into, &core);
                    if let Some(error) = failed {
                        bench.notify(toast::error(error), cx);
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }
}

/// The menu of a row of the tree, or of the tree itself when `path` is the project's folder.
fn row_menu(
    popup: PopupMenu,
    bench: &WeakEntity<Workbench>,
    path: &Path,
    dir: bool,
    root: Option<&Path>,
    here: bool,
    cx: &gpui_kit::App,
) -> PopupMenu {
    let whole = root == Some(path);
    let on = |work: fn(&mut Workbench, &Path, bool, &mut Window, &mut Context<Workbench>)| {
        let (bench, path) = (bench.clone(), path.to_owned());
        move |_: &ClickEvent, window: &mut Window, cx: &mut gpui_kit::App| {
            let _ = bench.update(cx, |bench, cx| work(bench, &path, dir, window, cx));
        }
    };
    let mut popup = if dir {
        popup
            .item(
                PopupMenuItem::new("New File")
                    .icon(Icon::FilePlus)
                    .on_click(on(|bench, path, _, window, cx| {
                        bench.files.selected_dir = Some(path.to_owned());
                        bench.name_new_entry(false, window, cx);
                    })),
            )
            .item(
                PopupMenuItem::new("New Folder")
                    .icon(Icon::FolderPlus)
                    .on_click(on(|bench, path, _, window, cx| {
                        bench.files.selected_dir = Some(path.to_owned());
                        bench.name_new_entry(true, window, cx);
                    })),
            )
    } else {
        popup.item(
            PopupMenuItem::new("Open")
                .icon(Icon::File)
                .on_click(on(|bench, path, _, _, cx| bench.open_file(path, cx))),
        )
    };
    if !whole {
        popup = popup
            .separator()
            .item(
                PopupMenuItem::new("Rename…")
                    .icon(Icon::Pencil)
                    .on_click(on(|bench, path, dir, window, cx| {
                        bench.rename_entry(path, dir, window, cx)
                    })),
            )
            .item(
                ui::MenuRow::new("Delete")
                    .icon(Icon::Trash)
                    .danger(true)
                    .item(cx)
                    .on_click(on(|bench, path, _, _, cx| {
                        bench.delete_entry(path.to_owned(), cx)
                    })),
            );
    }
    popup = popup.separator();
    if here {
        let revealed = path.to_owned();
        popup = popup.item(
            PopupMenuItem::new(ui::REVEAL)
                .icon(Icon::Folder)
                .on_click(move |_, _, cx| cx.reveal_path(&revealed)),
        );
    }
    let relative = root
        .and_then(|root| path.strip_prefix(root).ok())
        .map(|relative| relative.to_string_lossy().into_owned());
    let mut copies = vec![(
        "Copy Path",
        path.to_string_lossy().into_owned(),
        "Copied path",
    )];
    if let Some(relative) = relative.filter(|_| !whole) {
        copies.push(("Copy Relative Path", relative, "Copied relative path"));
    }
    for (label, copied, said) in copies {
        popup = popup.item(PopupMenuItem::new(label).icon(Icon::Copy).on_click(
            move |_, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
                window.push_notification(toast::success(said), cx);
            },
        ));
    }
    popup
}
