//! The names of files a chat's messages hold: where each points from the chat's folder,
//! whether the chat's host holds it, asked in one batch a frame, and what reaching for one
//! does, as on the Mac.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::WindowExt;
use gpui_kit::{App, ClipboardItem, Entity, Focusable, Pixels, Point, WeakEntity, Window};
use gpui_transcript::{FileKind, FilePress};

use sikemux_corekit::cli::protocol::CliOpenTarget;
use sikemux_corekit::cli::protocol::CliTargetKind;

use super::Reading;
use crate::core::Target;
use crate::hosts::Place;
use crate::icons::Icon;
use crate::retry::Backoff;
use crate::toast;
use crate::ui;
use crate::workbench::Workbench;
use sikemux_calls::call;
use sikemux_git::files::FileKind as CoreFileKind;
use sikemux_git::files::FileKindsParams;
use sikemux_markdown::paths::{normalized, parse, PathRef};

/// How long the names a screenful asks about are gathered before they go out as one ask.
const GATHER: Duration = Duration::from_millis(16);
/// The most names one ask may carry.
const MOST_ASKED: usize = 256;
/// A name the agent is about to write is missing when first mentioned, so a miss is only
/// trusted this long. A hit never goes stale.
const MISS_TRUSTED: Duration = Duration::from_secs(5);
const MOST_KNOWN: usize = 2_000;
/// The longest a chat waits to ask again after its host failed to answer.
const LONGEST_REST: Duration = Duration::from_secs(60);
/// What a chat knows of the names its messages hold, shared by the drawing of its rows.
#[derive(Default)]
pub struct Files {
    cwd: String,
    target: Option<Target>,
    known: HashMap<String, (Option<FileKind>, Instant)>,
    waiting: Vec<String>,
    gathering: bool,
    reader: Option<WeakEntity<Reading>>,
    /// The home folder of the chat's host, once known.
    home: Option<String>,
    /// After the host failed to answer, how long the next wait is and when this one ends.
    failing: Option<(Backoff, Instant)>,
}

impl Files {
    /// Points the names at the chat's folder on the chat's host, whose home is `home`, starting
    /// over when any of them moved.
    pub fn place(
        &mut self,
        cwd: &str,
        target: &Target,
        home: Option<String>,
        reader: &Entity<Reading>,
    ) {
        let moved = self.cwd != cwd
            || self.home != home
            || self
                .target
                .as_ref()
                .is_none_or(|held| held.place() != target.place());
        if moved {
            self.cwd = cwd.to_owned();
            self.home = home;
            self.target = Some(target.clone());
            self.known.clear();
            self.waiting.clear();
            self.failing = None;
        }
        self.reader = Some(reader.downgrade());
    }

    fn home(&self) -> Option<&str> {
        self.home.as_deref()
    }

    /// Where `name` points, given the chat's folder.
    pub fn resolve(&self, name: &str) -> Option<PathRef> {
        parse(name, &self.cwd, self.home())
    }

    /// Whether the chat's host holds `name`, asking it when that is not known.
    pub fn kind_of(files: &Rc<RefCell<Files>>, name: &str, cx: &mut App) -> Option<FileKind> {
        let mut held = files.borrow_mut();
        let path = held.resolve(name)?.path;
        match held.known.get(&path) {
            Some((Some(kind), _)) => return Some(*kind),
            Some((None, at)) if at.elapsed() < MISS_TRUSTED => return None,
            _ => {}
        }
        if held
            .failing
            .as_ref()
            .is_some_and(|(_, until)| Instant::now() < *until)
        {
            return None;
        }
        if !held.waiting.contains(&path) {
            held.waiting.push(path);
        }
        if !held.gathering {
            held.gathering = true;
            let files = files.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(GATHER).await;
                ask(files, cx).await;
            })
            .detach();
        }
        None
    }
}

/// Asks the chat's host about the names gathered, a batch at a time, then draws the rows again.
async fn ask(files: Rc<RefCell<Files>>, cx: &mut gpui_kit::AsyncApp) {
    loop {
        let (asked, target) = {
            let mut held = files.borrow_mut();
            let end = held.waiting.len().min(MOST_ASKED);
            let asked: Vec<String> = held.waiting.drain(..end).collect();
            if asked.is_empty() {
                held.gathering = false;
                return;
            }
            (asked, held.target.clone())
        };
        let Some(target) = target else {
            files.borrow_mut().gathering = false;
            return;
        };
        let Ok(kinds) = target
            .request_typed::<call::FilesKinds>(FileKindsParams {
                paths: asked.clone(),
            })
            .await
        else {
            let mut held = files.borrow_mut();
            let mut backoff = held.failing.take().map_or_else(
                || Backoff::new(MISS_TRUSTED, LONGEST_REST),
                |(backoff, _)| backoff,
            );
            let rest = backoff.next();
            held.failing = Some((backoff, Instant::now() + rest));
            held.waiting.clear();
            held.gathering = false;
            return;
        };
        let reader = {
            let mut held = files.borrow_mut();
            held.failing = None;
            for (ix, path) in asked.into_iter().enumerate() {
                let kind = match kinds.get(ix) {
                    Some(Some(CoreFileKind::Dir)) => Some(FileKind::Folder),
                    Some(Some(CoreFileKind::File)) => Some(FileKind::File),
                    _ => None,
                };
                held.known.insert(path, (kind, Instant::now()));
            }
            while held.known.len() > MOST_KNOWN {
                let oldest = held
                    .known
                    .iter()
                    .min_by_key(|(_, (_, at))| *at)
                    .map(|(path, _)| path.clone());
                match oldest {
                    Some(path) => held.known.remove(&path),
                    None => break,
                };
            }
            held.reader.clone()
        };
        if let Some(reader) = reader {
            reader.update(cx, |_, cx| cx.notify()).ok();
        }
    }
}

/// What reaching for a file the chat names does, given its name as written.
pub type FilePressed = Rc<dyn Fn(&str, FilePress, &mut Window, &mut App)>;

/// What reaching for the file `name` of `agent`'s chat does: a click opens it on the agent's
/// desk, two clicks open it in the editor, and the menu offers what the file tree's does.
pub fn pressed(
    agent: &str,
    files: &Rc<RefCell<Files>>,
    bench: &WeakEntity<Workbench>,
    (name, kind, press): (&str, FileKind, FilePress),
    window: &mut Window,
    cx: &mut App,
) {
    let (found, on) = {
        let held = files.borrow();
        let on = On {
            cwd: held.cwd.clone(),
            target: held.target.clone(),
            here: held
                .target
                .as_ref()
                .is_none_or(|target| *target.place() == Place::This),
        };
        (held.resolve(name), on)
    };
    let Some(found) = found else {
        return;
    };
    match press {
        FilePress::Open if kind == FileKind::File && on.here => {
            open_on_desk(agent, &found, bench, cx)
        }
        FilePress::Open | FilePress::OpenTwice => open(&found, kind, &on, bench, cx),
        FilePress::Menu(at) => {
            let menu = menu(agent, found, kind, on, bench.clone(), window, cx);
            window.focus(&menu.focus_handle(cx), cx);
            bench
                .update(cx, |bench, cx| bench.show_file_menu(agent, at, menu, cx))
                .ok();
        }
    }
}

/// Opens a file the chat names in the editor on the chat's own host, its cursor on the line
/// the name gave, or shows a folder of this host in Finder.
fn open(found: &PathRef, kind: FileKind, on: &On, bench: &WeakEntity<Workbench>, cx: &mut App) {
    match kind {
        FileKind::Folder if on.here => cx.reveal_path(Path::new(&found.path)),
        FileKind::Folder => {}
        FileKind::File => {
            let Some(target) = on.target.clone() else {
                return;
            };
            let asked = CliOpenTarget {
                id: found.path.clone(),
                kind: CliTargetKind::File,
                path: found.path.clone(),
                project_root: on.cwd.clone(),
                line: found.line,
                column: found.column,
            };
            let opened = bench.update(cx, |bench, cx| {
                bench.open_targets_on(&target, vec![asked], false, cx)
            });
            if let Ok(opened) = opened {
                crate::desk::spawn(async move {
                    for failed in opened.await.0.failed {
                        eprintln!(
                            "sikemux: the core did not open a file the chat names: {failed:?}"
                        );
                    }
                });
            }
        }
    }
}

/// Opens a file of this host's chat on its agent's desk, its cursor on the line the name gave,
/// and turns the tab rail to the file tree opened down to it when the tree holds it.
fn open_on_desk(agent: &str, found: &PathRef, bench: &WeakEntity<Workbench>, cx: &mut App) {
    let path = std::path::PathBuf::from(&found.path);
    bench
        .update(cx, |bench, cx| {
            let in_tree = (bench.files.root.as_deref()).is_some_and(|root| path.starts_with(root));
            if in_tree {
                if let Some(folder) = path.parent() {
                    let core = bench.target();
                    bench.files.open_down_to(folder, &core);
                }
                bench.show_rail_page(sikemux_view::prefs::TabRailPage::Files, cx);
            }
            bench.open_file_on_desk(agent, path, found.line, found.column, cx);
            bench.parts.redraw_stage(cx);
            bench.parts.redraw(&bench.parts.tabs, cx);
        })
        .ok();
}

/// Where a chat's files are: its folder, its host, and whether that host is this one.
#[derive(Clone)]
struct On {
    cwd: String,
    target: Option<Target>,
    here: bool,
}

fn menu(
    agent: &str,
    found: PathRef,
    kind: FileKind,
    on: On,
    bench: WeakEntity<Workbench>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<PopupMenu> {
    let name = Path::new(&found.path).file_name().map_or_else(
        || found.path.clone(),
        |name| name.to_string_lossy().into_owned(),
    );
    let relative = found
        .path
        .strip_prefix(&format!("{}/", normalized(&on.cwd)))
        .map_or_else(|| name.clone(), str::to_owned);
    let copies = [
        (t!("Copy Path"), found.path.clone(), t!("Copied path")),
        (
            t!("Copy Relative Path"),
            relative,
            t!("Copied relative path"),
        ),
        (t!("Copy Name"), name, t!("Copied name")),
    ];
    let agent = agent.to_owned();
    PopupMenu::build(
        window,
        cx,
        ui::menu(move |mut popup, _, _| {
            let file = kind == FileKind::File;
            if file && on.here {
                let (found, bench, agent) = (found.clone(), bench.clone(), agent.clone());
                popup = popup.item(
                    PopupMenuItem::new(t!("Open on Desk"))
                        .icon(Icon::PanelRight)
                        .on_click(move |_, _, cx| open_on_desk(&agent, &found, &bench, cx)),
                );
            }
            if file {
                let (found, bench, on) = (found.clone(), bench.clone(), on.clone());
                let (label, glyph) = match on.here {
                    true => (t!("Open in Editor"), Icon::Editor),
                    false => (t!("Open"), Icon::File),
                };
                popup = popup
                    .item(
                        PopupMenuItem::new(label)
                            .icon(glyph)
                            .on_click(move |_, _, cx| open(&found, kind, &on, &bench, cx)),
                    )
                    .separator();
            }
            if on.here {
                let revealed = found.path.clone();
                popup = popup.item(
                    PopupMenuItem::new(crate::i18n::tr(ui::REVEAL))
                        .icon(Icon::Folder)
                        .on_click(move |_, _, cx| cx.reveal_path(Path::new(&revealed))),
                );
                if file {
                    let folder = Path::new(&found.path)
                        .parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_default();
                    popup = popup.item(
                        PopupMenuItem::new(t!("Open Containing Folder"))
                            .icon(Icon::Folder)
                            .on_click(move |_, _, cx| cx.reveal_path(&folder)),
                    );
                }
                popup = popup.separator();
            }
            for (label, copied, said) in copies.clone() {
                popup = popup.item(PopupMenuItem::new(label).icon(Icon::Copy).on_click(
                    move |_, window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
                        window.push_notification(toast::success(said), cx);
                    },
                ));
            }
            popup
        }),
    )
}

/// Where the menu of a file a message names stands, while it is open.
pub struct FileMenu {
    pub at: Point<Pixels>,
    pub menu: Entity<PopupMenu>,
    pub _dismissed: gpui_kit::Subscription,
}
