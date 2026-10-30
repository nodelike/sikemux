//! Language servers for the editor's files, run by the core: a file opens
//! with its language's server once the person trusts its project, stays in
//! step as it is edited, and answers where a symbol is defined, implemented
//! and used.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::str::FromStr;
use std::time::Duration;

use gpui_kit::component::button::Button;
use gpui_kit::component::input::{DefinitionProvider, Rope, RopeExt};
use gpui_kit::{App, AppContext, Context, Task, Window};
use lsp_types::{LocationLink, Position, Range, ShowDocumentParams, Uri};
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use super::FileKey;
use super::{Document, Notice, OpenAt};
use crate::core::{self, Ask};
use crate::hosts::Place as Host;
use crate::toast;
use crate::workbench::Workbench;

/// How long typing pauses before the server is sent the text.
const CHANGE_DEBOUNCE: Duration = Duration::from_millis(120);
/// What a symbol can lead to, asked in turn until one answers.
const KINDS: [&str; 4] = [
    "definition",
    "declaration",
    "typeDefinition",
    "implementation",
];

/// Language servers the core installs: the language, its program, its name, and whether
/// Homebrew is what installs it.
const INSTALLABLE: [(&str, &str, &str, bool); 2] = [
    ("go", "gopls", "Go LSP", false),
    ("java", "jdtls", "Java LSP", true),
];

/// A file open with a language server, named as the core keeps it.
pub(super) struct Served {
    project: String,
    language: String,
    path: String,
    /// What the core knows this window's hold on the open file by, to claim it back
    /// after reconnecting.
    handle: String,
    version: u32,
    /// Sends the text to the server once typing pauses.
    sync: Task<()>,
    /// The symbol last held under the pointer with ⌘, which a ⌘-click then acts on.
    hovered: Rc<Cell<Option<Position>>>,
}

impl Served {
    pub(super) fn document(&self) -> Value {
        json!({ "project": self.project, "language": self.language, "path": self.path })
    }
}

/// The server a file opens with, as the core names it, and the language id
/// the protocol knows the file by.
fn languages(path: &Path, cx: &App) -> Option<(String, String)> {
    let language = crate::file_icons::language(path.file_name()?.to_str()?, cx)?;
    Some(match language.as_str() {
        "jsx" => ("javascript".into(), "javascriptreact".into()),
        "tsx" => ("typescript".into(), "typescriptreact".into()),
        _ => (language.clone(), language),
    })
}

/// The folder of the open project holding `path`.
pub(super) fn project_of(bench: &Workbench, path: &Path) -> Option<String> {
    bench
        .projects()
        .iter()
        .filter(|project| path.starts_with(&project.path))
        .max_by_key(|project| project.path.as_os_str().len())
        .map(|project| project.path.to_string_lossy().into_owned())
}

/// Opens the file with its language's server when the core has one and the
/// person trusts the project, asking them first when they have not said.
pub(super) fn attach(
    bench: &mut Workbench,
    path: PathBuf,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let trust = project_of(bench, &path).and_then(|project| {
        bench
            .preferences()
            .language_server_trust
            .get(&project)
            .copied()
    });
    attach_trusted(bench, path, trust, window, cx);
}

/// Opens the file with its language's server as [`attach`] does, with the person's answer
/// for the project as `trust` rather than read from preferences that may not have caught up.
fn attach_trusted(
    bench: &mut Workbench,
    path: PathBuf,
    trust: Option<bool>,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let (Some((language, language_id)), Some(project)) =
        (languages(&path, cx), project_of(bench, &path))
    else {
        return;
    };
    let asks = bench.asks.clone();
    cx.spawn_in(window, async move |bench, cx| {
        let served = core::request(&asks, "lsp.languages", json!({}))
            .await
            .unwrap_or_default();
        let served = served
            .as_array()
            .is_some_and(|served| served.iter().any(|name| name.as_str() == Some(&language)));
        match trust {
            _ if !served => return,
            Some(false) => return,
            None => {
                let _ = bench.update(cx, |bench, cx| {
                    let key = FileKey::new(Host::This, path.clone());
                    super::change(&bench.editors, &key, cx, |document, _| {
                        document.notice = Some(Notice::Trust(project))
                    });
                });
                return;
            }
            Some(true) => {}
        }
        let started = json!({ "project": project, "language": language });
        if let Err(error) = core::request(&asks, "lsp.start", started).await {
            let _ = bench.update(cx, |bench, cx| report(bench, &language, error, path, cx));
            return;
        }
        let Ok(Some(content)) = bench.update(cx, |bench, cx| {
            (bench.editors.read(cx).open)
                .get(&FileKey::new(Host::This, path.clone()))
                .map(|document| document.state.read(cx).value().to_string())
        }) else {
            return;
        };
        let name = path.to_string_lossy().into_owned();
        let opened = json!({
            "project": project,
            "language": language,
            "path": name,
            "content": content,
            "languageId": language_id,
        });
        let handle = match core::request(&asks, "lsp.open", opened).await {
            Ok(opened) => opened["handle"].as_str().unwrap_or_default().to_owned(),
            Err(error) => {
                let _ = bench.update(cx, |bench, cx| report(bench, &language, error, path, cx));
                return;
            }
        };
        let _ = bench.update(cx, |bench, cx| {
            let (asks, this, current) = (bench.asks.clone(), cx.weak_entity(), path.clone());
            bench.editors.update(cx, |editors, cx| {
                let Some(document) = editors.open.get_mut(&FileKey::new(Host::This, path)) else {
                    return;
                };
                let served = Served {
                    project,
                    language,
                    path: name,
                    handle,
                    version: 1,
                    sync: Task::ready(()),
                    hovered: Rc::new(Cell::new(None)),
                };
                let found = Rc::new(RefCell::new(Vec::new()));
                let definitions = Rc::new(Definitions {
                    asks,
                    document: served.document(),
                    found: found.clone(),
                    hovered: served.hovered.clone(),
                });
                document.state.update(cx, |state, _| {
                    let lsp = state.lsp_mut();
                    lsp.definition_provider = Some(definitions);
                    lsp.show_document = Some(Rc::new(
                        move |params: &ShowDocumentParams, window: &mut Window, cx: &mut App| {
                            let found = found.borrow().clone();
                            if found.len() > 1 {
                                let places = found.iter().filter_map(place).collect();
                                let _ = this.update(cx, |bench, cx| {
                                    super::peek(
                                        bench,
                                        &FileKey::new(Host::This, current.clone()),
                                        "Definitions",
                                        places,
                                        cx,
                                    )
                                });
                                return true;
                            }
                            show_document(params, window, cx)
                        },
                    ));
                });
                document.served = Some(served);
            });
        });
    })
    .detach();
}

/// Says once why the file at `path` did not open with its language server, as the Mac does,
/// offering to install a missing server the core knows how to.
fn report(
    bench: &mut Workbench,
    language: &str,
    error: String,
    path: PathBuf,
    cx: &mut Context<Workbench>,
) {
    let said = format!("{language}:{error}");
    if !bench
        .editors
        .update(cx, |editors, _| editors.lsp_said.insert(said))
    {
        return;
    }
    let missing = INSTALLABLE.into_iter().find(|(name, bin, _, _)| {
        *name == language
            && error.starts_with("lsp-server-missing:")
            && error.contains(&format!("`{bin}`"))
    });
    let Some((_, bin, label, by_brew)) = missing else {
        bench.notify(toast::error(format!("LSP {language} open: {error}")), cx);
        return;
    };
    if !by_brew {
        return offer_install(bench, language, bin, label, path, cx);
    }
    let (asks, language) = (bench.asks.clone(), language.to_owned());
    cx.spawn(async move |bench, cx| {
        let tools = core::request(&asks, "hostTools", json!({})).await;
        let brew = tools.is_ok_and(|tools| tools["brew"] == true);
        let _ = bench.update(cx, |bench, cx| match brew {
            true => offer_install(bench, &language, bin, label, path, cx),
            false => bench.notify(
                toast::error(format!(
                    "{label} needs {bin}. Install the {bin} package with your system's package manager to enable go-to-definition and references."
                )),
                cx,
            ),
        });
    })
    .detach();
}

fn offer_install(
    bench: &mut Workbench,
    language: &str,
    bin: &'static str,
    label: &str,
    path: PathBuf,
    cx: &mut Context<Workbench>,
) {
    let (this, language) = (cx.weak_entity(), language.to_owned());
    let note = toast::error(format!(
        "{label} needs {bin}. Install it to enable go-to-definition and references."
    ))
    .action(move |_, _, cx| {
        let (this, language, path, note) =
            (this.clone(), language.clone(), path.clone(), cx.entity());
        Button::new("lsp-install")
            .label(format!("Install {bin}"))
            .on_click(move |_, window, cx| {
                note.update(cx, |note, cx| note.dismiss(window, cx));
                let (language, path) = (language.clone(), path.clone());
                let _ = this.update(cx, |bench, cx| {
                    install(bench, language, bin, path, window, cx)
                });
            })
    });
    bench.notify(note, cx);
}

/// Has the core install the language's server, then opens the file with it again.
fn install(
    bench: &mut Workbench,
    language: String,
    bin: &'static str,
    path: PathBuf,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let installing = language.clone();
    if !bench
        .editors
        .update(cx, |editors, _| editors.lsp_installing.insert(installing))
    {
        bench.notify(toast::info(format!("{bin} install already running")), cx);
        return;
    }
    bench.notify(toast::info(format!("Installing {bin}…")), cx);
    let asked = core::request(&bench.asks, "lsp.install", json!({ "language": language }));
    cx.spawn_in(window, async move |bench, cx| {
        let installed = asked.await;
        let _ = bench.update_in(cx, |bench, window, cx| {
            let landed = installed.is_ok();
            bench.editors.update(cx, |editors, _| {
                editors.lsp_installing.remove(&language);
                if landed {
                    editors.lsp_said.clear();
                }
            });
            match installed {
                Ok(at) => {
                    let at = at
                        .as_str()
                        .map(|at| format!(" at {at}"))
                        .unwrap_or_default();
                    bench.notify(toast::success(format!("Installed {bin}{at}")), cx);
                    attach(bench, path, window, cx);
                }
                Err(error) => {
                    bench.notify(toast::error(format!("{bin} install failed: {error}")), cx)
                }
            }
        });
    })
    .detach();
}

/// Claims back the files this window had open with language servers, as the core lets go
/// of those not claimed soon after a connection drops, and opens again any it no longer has.
pub(super) fn claim(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let handles: Vec<String> = (bench.editors.read(cx).open.values())
        .filter_map(|document| Some(document.served.as_ref()?.handle.clone()))
        .collect();
    if handles.is_empty() {
        return;
    }
    let asks = bench.asks.clone();
    cx.spawn(async move |bench, cx| {
        let claimed = core::request(&asks, "lsp.claim", json!({ "handles": handles })).await;
        let claimed: Vec<String> = claimed
            .ok()
            .and_then(|claimed| serde_json::from_value(claimed).ok())
            .unwrap_or_default();
        let _ = bench.update(cx, |bench, cx| {
            let (lost, window) = bench.editors.update(cx, |editors, _| {
                let mut lost = Vec::new();
                for (key, document) in &mut editors.open {
                    if (document.served.as_ref())
                        .is_some_and(|served| !claimed.contains(&served.handle))
                    {
                        document.served = None;
                        lost.push(key.path.clone());
                    }
                }
                (lost, editors.window)
            });
            let Some(window) = window else {
                return;
            };
            cx.spawn(async move |this, cx| {
                let _ = window.update(cx, |_, window, cx| {
                    let _ = this.update(cx, |bench, cx| {
                        for path in lost {
                            attach(bench, path, window, cx);
                        }
                    });
                });
            })
            .detach();
        });
    })
    .detach();
}

/// Sends the server the file's text once typing pauses.
pub(super) fn changed(document: &mut Document, asks: &UnboundedSender<Ask>, cx: &mut App) {
    let Some(served) = document.served.as_mut() else {
        return;
    };
    served.version += 1;
    let mut params = served.document();
    params["version"] = served.version.into();
    let (asks, state) = (asks.clone(), document.state.clone());
    served.sync = cx.spawn(async move |cx| {
        cx.background_executor().timer(CHANGE_DEBOUNCE).await;
        let content = state.read_with(cx, |state, _| state.value().to_string());
        params["content"] = content.into();
        let _ = core::request(&asks, "lsp.change", params).await;
    });
}

/// Tells the server the file was saved with `content`.
pub(super) fn saved(document: &Document, content: &str, asks: &UnboundedSender<Ask>) {
    if let Some(served) = &document.served {
        let mut params = served.document();
        params["content"] = content.into();
        let _ = asks.send(Ask::Call {
            method: "lsp.save",
            params,
        });
    }
}

/// Lets the server go of a file no editor holds any more.
pub(super) fn closed(document: &Document, asks: &UnboundedSender<Ask>) {
    if let Some(served) = &document.served {
        let _ = asks.send(Ask::Call {
            method: "lsp.close",
            params: served.document(),
        });
    }
}

/// Remembers whether the project's language servers may run, then opens the
/// file with its server when they may.
pub(super) fn trust(
    bench: &mut Workbench,
    path: PathBuf,
    project: String,
    allowed: bool,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    remember_trust(bench, project, allowed);
    let key = FileKey::new(Host::This, path.clone());
    bench.editors.update(cx, |editors, _| {
        if let Some(document) = editors.open.get_mut(&key) {
            document.notice = None;
        }
    });
    if allowed {
        attach_trusted(bench, path, Some(true), window, cx);
    }
}

fn remember_trust(bench: &Workbench, project: String, allowed: bool) {
    let mut trusted = bench.preferences().language_server_trust.clone();
    trusted.insert(project, allowed);
    let _ = bench.asks.send(Ask::Call {
        method: "setPreference",
        params: json!({ "key": "languageServerTrust", "value": trusted }),
    });
}

/// Whether the person let `project`'s language servers run.
pub(crate) fn allowed(bench: &Workbench, project: &str) -> bool {
    bench
        .preferences()
        .language_server_trust
        .get(project)
        .is_some_and(|allowed| *allowed)
}

/// Lets `project`'s language servers start when a file there opens, or stops them now and
/// keeps them stopped.
pub(crate) fn allow(bench: &Workbench, project: String, allowed: bool) {
    if !allowed {
        let _ = bench.asks.send(Ask::Call {
            method: "lsp.stop",
            params: json!({ "project": project }),
        });
    }
    remember_trust(bench, project, allowed);
}

/// Opens a definition through editor.openAt, so ⌘[ comes back from it.
fn show_document(params: &ShowDocumentParams, window: &mut Window, cx: &mut App) -> bool {
    let Some(path) = file_path(&params.uri) else {
        return false;
    };
    let start = params
        .selection
        .map(|range| range.start)
        .unwrap_or_default();
    let at = OpenAt {
        path,
        line: start.line,
        column: Some(start.character),
    };
    window.dispatch_action(Box::new(at), cx);
    true
}

fn place(link: &LocationLink) -> Option<OpenAt> {
    let start = link.target_selection_range.start;
    Some(OpenAt {
        path: file_path(&link.target_uri)?,
        line: start.line,
        column: Some(start.character),
    })
}

/// The file a `file://` URI names, its escapes undone.
fn file_path(uri: &Uri) -> Option<PathBuf> {
    if uri.scheme().map(|scheme| scheme.as_str()) != Some("file") {
        return None;
    }
    let escaped = uri.path().as_str().as_bytes();
    let mut bytes = Vec::with_capacity(escaped.len());
    let mut at = 0;
    while at < escaped.len() {
        let hex = escaped
            .get(at + 1..at + 3)
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (escaped[at], hex) {
            (b'%', Some(byte)) => {
                bytes.push(byte);
                at += 3;
            }
            (byte, _) => {
                bytes.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8(bytes).ok().map(PathBuf::from)
}

/// Where a symbol leads, as the core's language server says. The places
/// last found are kept, since the editor jumps to the first of them alone.
struct Definitions {
    asks: UnboundedSender<Ask>,
    document: Value,
    found: Rc<RefCell<Vec<LocationLink>>>,
    hovered: Rc<Cell<Option<Position>>>,
}

impl DefinitionProvider for Definitions {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Vec<LocationLink>>> {
        let position = text.offset_to_position(offset);
        self.hovered.set(Some(position));
        let (asks, document) = (self.asks.clone(), self.document.clone());
        let asked = cx.background_spawn(async move {
            for kind in KINDS {
                let mut params = document.clone();
                params["line"] = position.line.into();
                params["character"] = position.character.into();
                params["kind"] = kind.into();
                let found = core::request(&asks, "lsp.locations", params)
                    .await
                    .map_err(anyhow::Error::msg)?;
                let links: Vec<LocationLink> = found
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(link)
                    .collect();
                if !links.is_empty() {
                    return Ok(links);
                }
            }
            anyhow::Ok(Vec::new())
        });
        let found = self.found.clone();
        cx.spawn(async move |_| {
            let links = asked.await?;
            found.replace(links.clone());
            Ok(links)
        })
    }
}

/// Where a ⌘⇧-click's symbol is used, or a ⌥⌘-click's is implemented, as on the Mac: uses are
/// always listed, and one implementation is gone to. Says whether the click was the server's.
pub(super) fn navigate(
    bench: &mut Workbench,
    key: &FileKey,
    references: bool,
    cx: &mut Context<Workbench>,
) -> bool {
    let Some(served) =
        (bench.editors.read(cx).open.get(key)).and_then(|document| document.served.as_ref())
    else {
        return false;
    };
    let Some(position) = served.hovered.get() else {
        return false;
    };
    let kind = if references {
        "references"
    } else {
        "implementation"
    };
    let mut params = served.document();
    params["line"] = position.line.into();
    params["character"] = position.character.into();
    params["kind"] = kind.into();
    let asked = core::request(&bench.asks, "lsp.locations", params);
    let key = key.clone();
    cx.spawn(async move |bench, cx| {
        let found = asked.await.unwrap_or_default();
        let mut places: Vec<OpenAt> = found
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(link)
            .filter_map(|link| place(&link))
            .collect();
        places.dedup_by(|a, b| a.path == b.path && a.line == b.line && a.column == b.column);
        let _ = bench.update(cx, |bench, cx| match (references, places.len()) {
            (_, 0) => bench.notify(toast::info(format!("No {kind} found")), cx),
            (false, 1) => super::go(bench, places.remove(0), cx),
            (true, _) => super::peek(bench, &key, "References", places, cx),
            (false, _) => super::peek(bench, &key, "Implementations", places, cx),
        });
    })
    .detach();
    true
}

fn link(location: &Value) -> Option<LocationLink> {
    let target_uri = Uri::from_str(location["uri"].as_str()?).ok()?;
    let range: Range = serde_json::from_value(location["range"].clone()).ok()?;
    Some(LocationLink {
        origin_selection_range: None,
        target_uri,
        target_range: range,
        target_selection_range: range,
    })
}
