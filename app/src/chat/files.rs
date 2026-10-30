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
use serde_json::{json, Value};

use sikemux_core::cli::protocol::{CliOpenTarget, CliTargetKind};
use sikemux_markdown::paths::{has_scheme, is_file_name, is_windows_drive};

use super::Reading;
use crate::core::Target;
use crate::hosts::Place;
use crate::icons::Icon;
use crate::retry::Backoff;
use crate::toast;
use crate::ui;
use crate::workbench::Workbench;

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
/// A reference longer than this is prose that happens to hold slashes.
const LONGEST: usize = 512;

/// A reference to a file, resolved far enough to open it.
#[derive(Clone, Debug, PartialEq)]
pub struct PathRef {
    /// Absolute, with `/` between its parts.
    pub path: String,
    /// Counted from 1, as a reader counts, when the reference named one.
    pub line: Option<u32>,
    pub column: Option<u32>,
}

/// `path:12`, `path:12:5` and `path#L12C5`, as a tool, a compiler and a code host each write
/// the same place.
fn split_place(raw: &str) -> (&str, Option<u32>, Option<u32>) {
    if let Some(at) = raw.rfind("#L").or_else(|| raw.rfind("#l")) {
        let place = &raw[at + 2..];
        let (line, column) = match place.find(['C', 'c']) {
            Some(c) => (&place[..c], Some(&place[c + 1..])),
            None => (place, None),
        };
        let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
        if digits(line) && column.is_none_or(digits) {
            return (
                &raw[..at],
                line.parse().ok(),
                column.and_then(|column| column.parse().ok()),
            );
        }
    }
    let number = |text: &str| -> Option<u32> {
        (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
            .then(|| text.parse().ok())
            .flatten()
    };
    if let Some((rest, last)) = raw.rsplit_once(':') {
        if let Some(last) = number(last) {
            if let Some((body, line)) = rest.rsplit_once(':') {
                if let Some(line) = number(line) {
                    return (body, Some(line), Some(last));
                }
            }
            return (rest, Some(last), None);
        }
    }
    (raw, None, None)
}

fn normalized(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_owned()
}

/// A bare name only refers to a file when it carries an extension or is one a project
/// spells out in full; anything else needs a separator.
fn looks_relative(body: &str) -> bool {
    body.contains('/') || body.contains('\\') || body.starts_with('.') || is_file_name(body)
}

fn file_uri_path(uri: &str) -> Option<String> {
    let url = url::Url::parse(uri)
        .ok()
        .filter(|url| url.scheme() == "file")?;
    let path = percent_decoded(url.path())?;
    match url
        .host_str()
        .filter(|host| !host.eq_ignore_ascii_case("localhost"))
    {
        Some(host) => Some(format!("//{host}{path}")),
        None => match path.strip_prefix('/').filter(|rest| is_windows_drive(rest)) {
            Some(drive) => Some(drive.to_owned()),
            None => Some(path),
        },
    }
}

fn percent_decoded(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = std::str::from_utf8(bytes.get(at + 1..at + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Reads what an agent wrote as a file reference, or none when it is not shaped like one.
/// Whether it is there is a separate question.
pub fn parse(raw: &str, cwd: &str, home: Option<&str>) -> Option<PathRef> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > LONGEST {
        return None;
    }
    let (body, line, column) = split_place(trimmed);
    if trimmed
        .get(..7)
        .is_some_and(|start| start.eq_ignore_ascii_case("file://"))
    {
        let path = file_uri_path(body)?;
        return Some(PathRef {
            path: normalized(&path),
            line,
            column,
        });
    }
    if body.is_empty() || (has_scheme(body) && !is_windows_drive(body)) {
        return None;
    }
    let expanded = match home {
        Some(home) if body == "~" => home.to_owned(),
        Some(home) if body.starts_with("~/") || body.starts_with("~\\") => {
            format!("{home}{}", &body[1..])
        }
        _ => body.to_owned(),
    };
    let absolute = expanded.starts_with('/') || is_windows_drive(&expanded);
    if !absolute && (body.contains(char::is_whitespace) || !looks_relative(&expanded)) {
        return None;
    }
    if !absolute && cwd.is_empty() {
        return None;
    }
    let path = if absolute {
        normalized(&expanded)
    } else {
        let rest = expanded.strip_prefix("./").unwrap_or(&expanded);
        let rest = rest.replace('\\', "/");
        [normalized(cwd), rest.trim_matches('/').to_owned()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("/")
    };
    (!path.is_empty()).then_some(PathRef { path, line, column })
}

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

    /// `path` as the chat would write it: within its folder from there, else from home.
    pub fn shown(&self, path: &str) -> String {
        let within = |root: &str| {
            path.strip_prefix(root)
                .and_then(|rest| rest.strip_prefix('/'))
                .map(str::to_owned)
        };
        within(&self.cwd).unwrap_or_else(|| crate::ui::path::tilde(path, self.home()))
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
            .request("files.kinds", json!({ "paths": asked }))
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
                let kind = match kinds.get(ix).and_then(Value::as_str) {
                    Some("dir") => Some(FileKind::Folder),
                    Some("file") => Some(FileKind::File),
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
                bench.show_rail_page(crate::workbench::RailPage::Files, cx);
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
        ("Copy Path", found.path.clone(), "Copied path"),
        ("Copy Relative Path", relative, "Copied relative path"),
        ("Copy Name", name, "Copied name"),
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
                    PopupMenuItem::new("Open on Desk")
                        .icon(Icon::PanelRight)
                        .on_click(move |_, _, cx| open_on_desk(&agent, &found, &bench, cx)),
                );
            }
            if file {
                let (found, bench, on) = (found.clone(), bench.clone(), on.clone());
                let (label, glyph) = match on.here {
                    true => ("Open in Editor", Icon::Editor),
                    false => ("Open", Icon::File),
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
                    PopupMenuItem::new(ui::REVEAL)
                        .icon(Icon::Folder)
                        .on_click(move |_, _, cx| cx.reveal_path(Path::new(&revealed))),
                );
                if file {
                    let folder = Path::new(&found.path)
                        .parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_default();
                    popup = popup.item(
                        PopupMenuItem::new("Open Containing Folder")
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

#[cfg(test)]
mod tests {
    use super::*;

    fn at(raw: &str) -> Option<String> {
        parse(raw, "/work/demo", Some("/Users/pat")).map(|found| found.path)
    }

    fn place(raw: &str) -> Option<(String, Option<u32>, Option<u32>)> {
        parse(raw, "/work/demo", Some("/Users/pat"))
            .map(|found| (found.path, found.line, found.column))
    }

    #[test]
    fn reads_references_as_the_mac_app_does() {
        assert_eq!(
            at("src/chat/AgentChatPane.tsx").as_deref(),
            Some("/work/demo/src/chat/AgentChatPane.tsx")
        );
        assert_eq!(at("/etc/hosts").as_deref(), Some("/etc/hosts"));
        assert_eq!(
            place("src/a.ts:42:7"),
            Some(("/work/demo/src/a.ts".into(), Some(42), Some(7)))
        );
        assert_eq!(place("src/a.ts:42").and_then(|found| found.1), Some(42));
        assert_eq!(
            place("file:///work/demo/src/a.ts#L9"),
            Some(("/work/demo/src/a.ts".into(), Some(9), None))
        );
        assert_eq!(
            at("file:///work/demo/my%20notes.md").as_deref(),
            Some("/work/demo/my notes.md")
        );
        assert_eq!(
            at("~/notes/todo.md").as_deref(),
            Some("/Users/pat/notes/todo.md")
        );
        assert_eq!(at("./src/a.ts").as_deref(), Some("/work/demo/src/a.ts"));
        assert_eq!(at("https://example.com/a/b.ts"), None);
        assert_eq!(at("mailto:pat@example.com"), None);
        assert_eq!(at("finally"), None);
        assert_eq!(at("run the build"), None);
        assert_eq!(
            at("package.json").as_deref(),
            Some("/work/demo/package.json")
        );
        assert_eq!(at("Makefile").as_deref(), Some("/work/demo/Makefile"));
        assert_eq!(
            at("/work/demo/Shipped Mon 5 + Wed 7.mp4").as_deref(),
            Some("/work/demo/Shipped Mon 5 + Wed 7.mp4")
        );
        assert_eq!(
            at("~/Movies/demo cut.mp4").as_deref(),
            Some("/Users/pat/Movies/demo cut.mp4")
        );
        assert_eq!(at("demo cut.mp4"), None);
        assert_eq!(
            at("C:\\work\\demo\\a.ts").as_deref(),
            Some("C:/work/demo/a.ts")
        );
        assert_eq!(
            place("src/a.ts#L9C4"),
            Some(("/work/demo/src/a.ts".into(), Some(9), Some(4)))
        );
        assert_eq!(
            place("file:///C:/work/a.ts:3"),
            Some(("C:/work/a.ts".into(), Some(3), None))
        );
        assert_eq!(at("file:///work/%E0%A4%A.ts"), None);
        assert_eq!(at("   "), None);
        assert_eq!(at(&format!("src/{}.ts", "a".repeat(600))), None);
        assert_eq!(at(":12"), None);
        assert_eq!(at(".env").as_deref(), Some("/work/demo/.env"));
        assert_eq!(at(".gitignore").as_deref(), Some("/work/demo/.gitignore"));
        assert_eq!(parse("src/a.ts", "", None), None);
        assert_eq!(
            parse("/etc/hosts", "", None)
                .map(|found| found.path)
                .as_deref(),
            Some("/etc/hosts")
        );
    }
}
