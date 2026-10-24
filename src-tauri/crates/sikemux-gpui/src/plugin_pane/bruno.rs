//! The Bruno pane: a collection's requests as a tree, each opened as a tab to edit, save and
//! send, with what came back beside it. The plugin reads and writes the files and runs the
//! requests; the pane holds only what is being typed.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::f32::consts::FRAC_PI_2;
use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState, Textarea};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, radians, AnyElement, App, AppContext, Context, Div, ElementId, Entity, FontWeight,
    Hsla, InteractiveElement, IntoElement, ParentElement, PathPromptOptions, SharedString,
    Stateful, StatefulInteractiveElement, Styled, Subscription, Transformation, Window,
};
use serde_json::{json, Value};
use sikemux_wire::bruno::{
    AuthMode, BodyMode, Collection, Env, HttpMethod, KeyPlacement, KnownWorkspaces, Request,
    RequestView, RunResult, TreeNode,
};

use super::{after, listen, Host, Panel, PluginPane};
use crate::icons::Icon;
use crate::theme::{self, Tokens, RADIUS_2};
use crate::ui::{self, Button, IconButton, Pick, Segmented, Variant};

mod editor;

use editor::{Editor, Table, Text};

pub const REGISTRATION: super::Registration = super::Registration {
    plugin: "sikemux.bruno",
    surface: "client",
    pane: super::boxed::<Bruno>,
    top_bar: None,
    palettes: PALETTES,
};

/// Bruno's palettes: a request to open, an environment to send with, a workspace to switch to.
const PALETTES: &[super::PluginPalette] = &[
    super::PluginPalette {
        plugin: "sikemux.bruno",
        id: "bruno.requests",
        title: "Find a Bruno request",
        placeholder: "search requests…",
        view: "collectionView",
        params: Some(collection_params),
        rows: request_rows,
        typed: None,
    },
    super::PluginPalette {
        plugin: "sikemux.bruno",
        id: "bruno.environments",
        title: "Pick a Bruno environment",
        placeholder: "search environments…",
        view: "collectionView",
        params: Some(collection_params),
        rows: environment_rows,
        typed: None,
    },
    super::PluginPalette {
        plugin: "sikemux.bruno",
        id: "bruno.workspaces",
        title: "Switch Bruno workspace",
        placeholder: "switch bruno workspace…",
        view: "workspacesView",
        params: Some(workspace_params),
        rows: workspace_rows,
        typed: None,
    },
];

fn collection_params(settings: &Value) -> Value {
    json!({ "root": settings["collectionPath"] })
}

fn workspace_params(settings: &Value) -> Value {
    json!({ "known": settings["workspaces"], "loaded": settings["collectionPath"] })
}

fn request_rows(collection: &Value) -> Vec<crate::palette::Row> {
    fn walk(nodes: &[TreeNode], trail: &mut Vec<String>, rows: &mut Vec<crate::palette::Row>) {
        for node in nodes {
            match node {
                TreeNode::Folder { name, children, .. } => {
                    trail.push(name.clone());
                    walk(children, trail, rows);
                    trail.pop();
                }
                TreeNode::Request { name, path, method, .. } => {
                    let folders = trail.join(" / ");
                    let meta = match folders.is_empty() {
                        true => method_word(*method).to_owned(),
                        false => format!("{} · {folders}", method_word(*method)),
                    };
                    rows.push(crate::palette::Row::new(format!("request:{path}"), name.clone()).meta(meta).also(&folders));
                }
            }
        }
    }
    let Ok(collection) = serde_json::from_value::<Collection>(collection.clone()) else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    walk(&collection.tree, &mut Vec::new(), &mut rows);
    rows
}

fn environment_rows(collection: &Value) -> Vec<crate::palette::Row> {
    let Ok(collection) = serde_json::from_value::<Collection>(collection.clone()) else {
        return Vec::new();
    };
    let mut owners: Vec<(String, String)> = Vec::new();
    for env in &collection.envs {
        if !owners.iter().any(|(path, _)| *path == env.collection_path) {
            owners.push((env.collection_path.clone(), env.collection_name.clone()));
        }
    }
    let none = owners.iter().map(|(path, name)| {
        crate::palette::Row::new(format!("env-none:{path}"), "No environment").meta(name.clone())
    });
    let envs = collection
        .envs
        .iter()
        .map(|env| crate::palette::Row::new(format!("env:{}", env.id), env.name.clone()).meta(env.collection_name.clone()));
    envs.chain(none).collect()
}

fn workspace_rows(known: &Value) -> Vec<crate::palette::Row> {
    let Ok(known) = serde_json::from_value::<KnownWorkspaces>(known.clone()) else {
        return Vec::new();
    };
    known
        .workspaces
        .into_iter()
        .map(|workspace| {
            crate::palette::Row::new(format!("workspace:{}", workspace.path), workspace.name).meta(workspace.path)
        })
        .chain([crate::palette::Row::new("workspace-add", "Add a folder…")])
        .collect()
}

const METHODS: [HttpMethod; 7] = [
    HttpMethod::Get,
    HttpMethod::Post,
    HttpMethod::Put,
    HttpMethod::Delete,
    HttpMethod::Patch,
    HttpMethod::Options,
    HttpMethod::Head,
];

const SECTIONS: [&str; 8] = ["Params", "Body", "Headers", "Auth", "Vars", "Assert", "Script", "Docs"];
const RESPONSE_TABS: [&str; 4] = ["Body", "Headers", "Timeline", "Tests"];
/// The most of a response body drawn at once.
const BODY_SHOWN: usize = 256 * 1024;

#[derive(Default)]
pub struct Bruno {
    /// Open requests, by file, in tab order.
    open: Vec<String>,
    front: Option<String>,
    /// Folders open in the tree.
    expanded: HashSet<String>,
    editors: HashMap<String, Editor>,
    /// Each open request as its file says it, last read.
    disk: HashMap<String, Request>,
    /// Requests whose fields differ from their file.
    drafts: HashMap<String, Request>,
    results: HashMap<String, Rc<RunResult>>,
    running: HashSet<String>,
    section: usize,
    response_tab: usize,
    /// Values earlier runs' scripts set in this session.
    runtime: BTreeMap<String, String>,
    secrets: BTreeMap<String, String>,
    secret_fields: HashMap<String, Entity<InputState>>,
    /// The collection the person trusted for this session.
    trusted: Option<String>,
    /// The request waiting for the person to trust its collection, with the environment it
    /// is to run in.
    asking: Option<(String, Option<String>)>,
    _typing: Vec<Subscription>,
}

fn method_word(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Delete => "DELETE",
        HttpMethod::Patch => "PATCH",
        HttpMethod::Options => "OPTIONS",
        HttpMethod::Head => "HEAD",
    }
}

fn method_ink(method: HttpMethod, t: &Tokens) -> Hsla {
    match method {
        HttpMethod::Get => t.live,
        HttpMethod::Post => t.accent,
        HttpMethod::Put | HttpMethod::Patch => t.warn,
        HttpMethod::Delete => t.danger,
        HttpMethod::Options | HttpMethod::Head => t.text_tertiary,
    }
}

fn base_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).trim_end_matches(".bru").to_owned()
}

fn find<'a>(nodes: &'a [TreeNode], path: &str) -> Option<&'a TreeNode> {
    nodes.iter().find_map(|node| match node {
        TreeNode::Request { path: here, .. } if here == path => Some(node),
        TreeNode::Request { .. } => None,
        TreeNode::Folder { children, .. } => find(children, path),
    })
}

impl Panel for Bruno {
    fn open_at(&mut self, at: &str, host: &mut Host, cx: &mut Context<PluginPane>) {
        if let Some(path) = at.strip_prefix("request:") {
            self.show(path);
        } else if let Some(id) = at.strip_prefix("env:") {
            let owner = id.rsplit_once("::").map_or(String::new(), |(owner, _)| owner.to_owned());
            let id = id.to_owned();
            host.save_settings(move |settings| choose_env(settings, owner, Some(id)), cx);
        } else if let Some(owner) = at.strip_prefix("env-none:") {
            let owner = owner.to_owned();
            host.save_settings(move |settings| choose_env(settings, owner, None), cx);
        } else if let Some(folder) = at.strip_prefix("workspace:") {
            *self = Bruno::default();
            open_collection(host, folder.to_owned(), cx);
        } else if at == "workspace-add" {
            choose_collection(cx);
        }
        cx.notify();
    }

    fn render(&mut self, host: &mut Host, window: &mut Window, cx: &mut Context<PluginPane>) -> AnyElement {
        let settings = host.settings(cx);
        let root = settings["collectionPath"].as_str().unwrap_or_default().to_owned();
        if root.is_empty() {
            return welcome(cx);
        }
        let read = host
            .view("collectionView", json!({ "root": root }), cx)
            .read::<Collection>();
        let collection = match read {
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(&failure, cx),
            Ok(Some(collection)) => collection,
        };
        let main = match self.front.clone() {
            None => super::empty("Pick a request in the tree to open it.", cx),
            Some(path) => self.request_page(&root, &path, &collection, host, window, cx),
        };
        div()
            .size_full()
            .flex()
            .child(self.tree(&collection, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .children((!self.open.is_empty()).then(|| self.strip(&collection, cx)))
                    .child(div().flex_1().min_h_0().child(main)),
            )
            .children(self.asking(&root, cx))
            .into_any_element()
    }
}

impl Bruno {
    fn show(&mut self, path: &str) {
        if !self.open.iter().any(|open| open == path) {
            self.open.push(path.to_owned());
        }
        self.front = Some(path.to_owned());
    }

    fn close(&mut self, path: &str) {
        let Some(at) = self.open.iter().position(|open| open == path) else {
            return;
        };
        self.open.remove(at);
        if self.front.as_deref() == Some(path) {
            self.front = self.open.get(at).or_else(|| at.checked_sub(1).and_then(|left| self.open.get(left))).cloned();
        }
        if !self.drafts.contains_key(path) {
            self.editors.remove(path);
            self.disk.remove(path);
        }
    }

    /// A field of the request at `path` changed: it is a draft while it differs from its file.
    fn edited(&mut self, path: &str, cx: &App) {
        let Some(editor) = self.editors.get(path) else {
            return;
        };
        let request = editor.request(cx);
        match self.disk.get(path) == Some(&request) {
            true => self.drafts.remove(path),
            false => self.drafts.insert(path.to_owned(), request),
        };
    }

    fn save(&mut self, host: &Host, root: &str, path: &str, cx: &mut Context<PluginPane>) {
        let Some(editor) = self.editors.get(path) else {
            return;
        };
        let request = editor.request(cx);
        let call = host.call("saveRequest", json!({ "root": root, "path": path, "request": request }));
        let path = path.to_owned();
        after::<Bruno, _>(
            call,
            move |bruno, host, done, cx| match done {
                Ok(_) => {
                    bruno.drafts.remove(&path);
                }
                Err(failure) => host.said(&failure, cx),
            },
            cx,
        );
    }

    fn send(&mut self, host: &Host, root: &str, path: &str, env: Option<String>, cx: &mut Context<PluginPane>) {
        if self.trusted.as_deref() != Some(root) {
            self.asking = Some((path.to_owned(), env));
            cx.notify();
            return;
        }
        let Some(editor) = self.editors.get(path) else {
            return;
        };
        if !self.running.insert(path.to_owned()) {
            return;
        }
        let asked = json!({
            "root": root,
            "path": path,
            "request": editor.request(cx),
            "env": env,
            "secrets": self.secrets,
            "runtime": self.runtime,
            "trusted": true,
        });
        let call = host.call("run", asked);
        let path = path.to_owned();
        after::<Bruno, _>(
            call,
            move |bruno, host, done, cx| {
                bruno.running.remove(&path);
                match done.map(serde_json::from_value::<RunResult>) {
                    Ok(Ok(result)) => {
                        for (name, value) in &result.env_updates {
                            if bruno.secrets.contains_key(name) {
                                bruno.secrets.insert(name.clone(), value.clone());
                            }
                            bruno.runtime.insert(name.clone(), value.clone());
                        }
                        bruno.results.insert(path, Rc::new(result));
                    }
                    Ok(Err(error)) => {
                        let failure = super::Failure {
                            category: "core".into(),
                            message: format!("unreadable run: {error}").into(),
                            status: None,
                        };
                        host.said(&failure, cx);
                    }
                    Err(failure) => host.said(&failure, cx),
                }
            },
            cx,
        );
        cx.notify();
    }

    fn tree(&self, collection: &Collection, cx: &mut Context<PluginPane>) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let mut rows = Vec::new();
        self.rows(&collection.tree, 0, &mut rows, cx);
        let head = div()
            .h(px(32.))
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(4.))
            .child(
                ui::text(collection.name.clone(), 12., FontWeight::SEMIBOLD)
                    .text_color(t.text_primary)
                    .flex_1()
                    .min_w_0()
                    .truncate(),
            )
            .child(
                IconButton::new("bruno-refresh", Icon::Refresh, "Read the collection again")
                    .small()
                    .on_click(listen::<Bruno, _>(cx, |_, host, _, _, _| host.read_again(None))),
            )
            .child(
                IconButton::new("bruno-workspace", Icon::Folder, "Open another collection")
                    .small()
                    .on_click(listen::<Bruno, _>(cx, |_, _, _, _, cx| choose_collection(cx))),
            );
        div()
            .w(px(260.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(t.border)
            .child(head)
            .child(
                div()
                    .id("bruno-tree")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(4.))
                    .flex()
                    .flex_col()
                    .children(rows),
            )
            .into_any_element()
    }

    fn rows(&self, nodes: &[TreeNode], depth: usize, rows: &mut Vec<AnyElement>, cx: &mut Context<PluginPane>) {
        let t = Tokens::get(cx).clone();
        for node in nodes {
            match node {
                TreeNode::Folder { name, path, children } => {
                    let open = self.expanded.contains(path);
                    let toggled = path.clone();
                    let chevron = ui::icon(Icon::Chevron, 9.)
                        .when(open, |chevron| chevron.with_transformation(Transformation::rotate(radians(FRAC_PI_2))))
                        .text_color(t.text_tertiary);
                    rows.push(
                        row(format!("bruno-folder/{path}"), depth, false, cx)
                            .child(div().w(px(10.)).flex_none().child(chevron))
                            .child(ui::icon(Icon::Folder, 13.).text_color(t.text_tertiary))
                            .child(label(name.clone(), &t))
                            .on_click(listen::<Bruno, _>(cx, move |bruno, _, _, _, cx| {
                                if !bruno.expanded.remove(&toggled) {
                                    bruno.expanded.insert(toggled.clone());
                                }
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                    if open {
                        self.rows(children, depth + 1, rows, cx);
                    }
                }
                TreeNode::Request { name, path, method, .. } => {
                    let current = self.front.as_deref() == Some(path.as_str());
                    let opening = path.clone();
                    let mark = match (self.running.contains(path), self.drafts.contains_key(path)) {
                        (true, _) => Some(ui::meta("…", cx).into_any_element()),
                        (false, true) => Some(ui::dot(ui::Tone::Pending, cx).into_any_element()),
                        (false, false) => None,
                    };
                    rows.push(
                        row(format!("bruno-request/{path}"), depth, current, cx)
                            .child(div().w(px(10.)).flex_none())
                            .child(
                                div()
                                    .w(px(44.))
                                    .flex_none()
                                    .font_family(theme::MONO)
                                    .text_size(theme::ui_text(10.))
                                    .text_color(method_ink(*method, &t))
                                    .child(method_word(*method)),
                            )
                            .child(label(name.clone(), &t))
                            .children(mark)
                            .on_click(listen::<Bruno, _>(cx, move |bruno, _, _, _, cx| {
                                bruno.show(&opening);
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
            }
        }
    }

    fn strip(&self, collection: &Collection, cx: &mut Context<PluginPane>) -> AnyElement {
        let t = Tokens::get(cx).clone();
        div()
            .id("bruno-tabs")
            .h(px(36.))
            .px(px(6.))
            .flex()
            .items_center()
            .gap(px(4.))
            .overflow_x_scroll()
            .border_b_1()
            .border_color(t.border)
            .children(self.open.iter().enumerate().map(|(at, path)| {
                let (name, method) = match find(&collection.tree, path) {
                    Some(TreeNode::Request { name, method, .. }) => (name.clone(), *method),
                    _ => (base_name(path), HttpMethod::Get),
                };
                let (picked, closed) = (path.clone(), path.clone());
                ui::Tab::new(("bruno-tab", at), name)
                    .mark(
                        div()
                            .font_family(theme::MONO)
                            .text_size(theme::ui_text(9.5))
                            .text_color(method_ink(method, &t))
                            .child(method_word(method)),
                    )
                    .note(self.drafts.contains_key(path).then(|| SharedString::from("edited")))
                    .active(self.front.as_deref() == Some(path.as_str()))
                    .on_click(listen::<Bruno, _>(cx, move |bruno, _, _, _, cx| {
                        bruno.front = Some(picked.clone());
                        cx.notify();
                    }))
                    .on_close(listen::<Bruno, _>(cx, move |bruno, _, _, _, cx| {
                        bruno.close(&closed);
                        cx.notify();
                    }))
            }))
            .into_any_element()
    }

    fn request_page(
        &mut self,
        root: &str,
        path: &str,
        collection: &Collection,
        host: &mut Host,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let read = host
            .view("requestView", json!({ "root": root, "path": path }), cx)
            .read::<RequestView>();
        let view = match read {
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(&failure, cx),
            Ok(Some(view)) => view,
        };
        let settings = host.settings(cx);
        let stale = self.disk.get(path) != Some(&view.request);
        self.disk.insert(path.to_owned(), view.request.clone());
        if !self.editors.contains_key(path) || (stale && !self.drafts.contains_key(path)) {
            let opened = self.drafts.get(path).cloned().unwrap_or_else(|| view.request.clone());
            let editor = Editor::new(path, &opened, window, cx);
            self.editors.insert(path.to_owned(), editor);
        }
        let envs: Vec<&Env> = collection
            .envs
            .iter()
            .filter(|env| view.collection_path.is_empty() || env.collection_path == view.collection_path)
            .collect();
        let chosen = settings["selectedEnvs"][view.collection_path.as_str()]
            .as_str()
            .filter(|id| envs.iter().any(|env| env.id == *id))
            .map(str::to_owned);
        let secret_names: Vec<String> = chosen
            .as_ref()
            .and_then(|id| envs.iter().find(|env| env.id == *id))
            .map(|env| env.secret_names.clone())
            .unwrap_or_default();
        self.secret_fields_for(&secret_names, window, cx);
        let t = Tokens::get(cx).clone();
        let Some(editor) = self.editors.get(path) else {
            return super::loading(cx);
        };
        let top = self.top_row((root, path), editor, (&envs, chosen.clone(), &view.collection_path), cx);
        let secrets = (!secret_names.is_empty()).then(|| {
            div()
                .px(px(12.))
                .pb(px(8.))
                .flex()
                .flex_wrap()
                .gap(px(8.))
                .children(secret_names.iter().filter_map(|name| {
                    let field = self.secret_fields.get(name)?;
                    Some(
                        div()
                            .w(px(220.))
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(ui::meta(format!("{name} (secret)"), cx))
                            .child(ui::Field::new(field)),
                    )
                }))
        });
        let sections = Segmented::words(
            "bruno-sections",
            SECTIONS.iter().map(|section| (SharedString::from(*section), None)).collect(),
            self.section,
        )
        .on_select(listen::<Bruno, usize>(cx, |bruno, _, picked, _, cx| {
            bruno.section = *picked;
            cx.notify();
        }));
        let body = self.section_body(path, editor, cx);
        let request_side = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .child(top)
            .children(secrets)
            .child(div().px(px(12.)).pb(px(8.)).child(sections))
            .child(
                div()
                    .id("bruno-section")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(12.))
                    .pb(px(12.))
                    .child(body),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(request_side)
            .child(
                div()
                    .h(gpui_kit::relative(0.42))
                    .flex_none()
                    .w_full()
                    .border_t_1()
                    .border_color(t.border)
                    .child(self.response(path, cx)),
            )
            .into_any_element()
    }

    fn secret_fields_for(&mut self, names: &[String], window: &mut Window, cx: &mut Context<PluginPane>) {
        for name in names {
            if self.secret_fields.contains_key(name) {
                continue;
            }
            let value = self.secrets.get(name).cloned().unwrap_or_default();
            let field = cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder("typed for this session only")
                    .default_value(value)
            });
            let named = name.clone();
            self._typing.push(cx.subscribe(&field, move |pane, field, event: &InputEvent, cx| {
                if !matches!(event, InputEvent::Change) {
                    return;
                }
                let typed = field.read(cx).value().to_string();
                if let Some((bruno, _)) = pane.parts::<Bruno>() {
                    bruno.secrets.insert(named.clone(), typed);
                }
            }));
            self.secret_fields.insert(name.clone(), field);
        }
    }

    /// The method, the address, the environment, Save and Send. `at` is the collection and the
    /// request's file; `picked` the environments to choose from, the one chosen, and whose.
    fn top_row(
        &self,
        at: (&str, &str),
        editor: &Editor,
        picked: (&[&Env], Option<String>, &str),
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let (root, path) = at;
        let (envs, chosen, collection_path) = picked;
        let edited_path = path.to_owned();
        let method = ui::select(
            "bruno-method",
            method_word(editor.method),
            METHODS.iter().map(|method| Pick::new(method_word(*method), method_word(*method))).collect(),
            Some(method_word(editor.method).into()),
            false,
            listen::<Bruno, SharedString>(cx, move |bruno, _, picked, _, cx| {
                if let Some(editor) = bruno.editors.get_mut(&edited_path) {
                    if let Some(method) = METHODS.iter().find(|method| method_word(**method) == picked.as_ref()) {
                        editor.method = *method;
                    }
                }
                bruno.edited(&edited_path, cx);
                cx.notify();
            }),
            cx,
        );
        let mut env_picks = vec![Pick::new("", "No environment")];
        env_picks.extend(envs.iter().map(|env| Pick::new(env.id.clone(), env.name.clone())));
        let shown = chosen
            .as_ref()
            .and_then(|id| envs.iter().find(|env| env.id == *id))
            .map_or("No environment".to_owned(), |env| env.name.clone());
        let owner = collection_path.to_owned();
        let env = ui::select(
            "bruno-env",
            shown,
            env_picks,
            Some(chosen.clone().unwrap_or_default().into()),
            false,
            listen::<Bruno, SharedString>(cx, move |_, host, picked, _, cx| {
                let (owner, picked) = (owner.clone(), picked.to_string());
                let picked = (!picked.is_empty()).then_some(picked);
                host.save_settings(move |settings| choose_env(settings, owner, picked), cx);
            }),
            cx,
        );
        let running = self.running.contains(path);
        let dirty = self.drafts.contains_key(path);
        let (send_root, send_path, save_root, save_path) = (root.to_owned(), path.to_owned(), root.to_owned(), path.to_owned());
        div()
            .px(px(12.))
            .py(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .child(method)
            .child(div().flex_1().min_w_0().child(ui::Field::new(&editor.url).mono()))
            .child(env)
            .children(dirty.then(|| {
                Button::new("bruno-save", "Save", Variant::Outline).on_click(listen::<Bruno, _>(
                    cx,
                    move |bruno, host, _, _, cx| bruno.save(host, &save_root, &save_path, cx),
                ))
            }))
            .child(
                Button::new("bruno-send", if running { "Sending…" } else { "Send" }, Variant::Primary)
                    .disabled(running)
                    .on_click(listen::<Bruno, _>(cx, move |bruno, host, _, _, cx| {
                        let env = chosen.clone();
                        bruno.send(host, &send_root, &send_path, env, cx)
                    })),
            )
            .into_any_element()
    }

    fn section_body(&self, path: &str, editor: &Editor, cx: &mut Context<PluginPane>) -> AnyElement {
        let column = div().flex().flex_col().gap(px(10.));
        match SECTIONS.get(self.section).copied().unwrap_or("Params") {
            "Params" => column
                .child(heading("Query", cx))
                .child(table(path, editor, Table::Query, cx))
                .child(heading("Path", cx))
                .child(table(path, editor, Table::Path, cx))
                .into_any_element(),
            "Body" => self.body_section(path, editor, cx),
            "Headers" => column.child(table(path, editor, Table::Headers, cx)).into_any_element(),
            "Auth" => self.auth_section(path, editor, cx),
            "Vars" => column
                .child(heading("Before the request", cx))
                .child(table(path, editor, Table::RequestVars, cx))
                .child(heading("From the response", cx))
                .child(table(path, editor, Table::ResponseVars, cx))
                .into_any_element(),
            "Assert" => column.child(table(path, editor, Table::Assertions, cx)).into_any_element(),
            "Script" => column
                .child(heading("Before the request", cx))
                .child(code(editor, Text::PreScript, cx))
                .child(heading("After the response", cx))
                .child(code(editor, Text::PostScript, cx))
                .into_any_element(),
            _ => column.child(code(editor, Text::Docs, cx)).into_any_element(),
        }
    }

    fn body_section(&self, path: &str, editor: &Editor, cx: &mut Context<PluginPane>) -> AnyElement {
        const MODES: [(BodyMode, &str); 9] = [
            (BodyMode::None, "No body"),
            (BodyMode::Json, "JSON"),
            (BodyMode::Text, "Text"),
            (BodyMode::Xml, "XML"),
            (BodyMode::Sparql, "SPARQL"),
            (BodyMode::Graphql, "GraphQL"),
            (BodyMode::FormUrlencoded, "Form URL encoded"),
            (BodyMode::MultipartForm, "Multipart form"),
            (BodyMode::File, "File"),
        ];
        let label_of = |mode: BodyMode| MODES.iter().find(|(each, _)| *each == mode).map_or("No body", |(_, label)| label);
        let edited_path = path.to_owned();
        let mode = ui::select(
            "bruno-body-mode",
            label_of(editor.body_mode),
            MODES.iter().map(|(_, label)| Pick::new(*label, *label)).collect(),
            Some(label_of(editor.body_mode).into()),
            false,
            listen::<Bruno, SharedString>(cx, move |bruno, _, picked, _, cx| {
                if let Some(editor) = bruno.editors.get_mut(&edited_path) {
                    if let Some((mode, _)) = MODES.iter().find(|(_, label)| *label == picked.as_ref()) {
                        editor.body_mode = *mode;
                    }
                }
                bruno.edited(&edited_path, cx);
                cx.notify();
            }),
            cx,
        );
        let content = match editor.body_mode {
            BodyMode::None => None,
            BodyMode::FormUrlencoded | BodyMode::MultipartForm | BodyMode::File => Some(table(path, editor, Table::Form, cx)),
            _ => Some(code(editor, Text::Body, cx)),
        };
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(div().flex().child(mode))
            .children(content)
            .into_any_element()
    }

    fn auth_section(&self, path: &str, editor: &Editor, cx: &mut Context<PluginPane>) -> AnyElement {
        const MODES: [(AuthMode, &str); 6] = [
            (AuthMode::Inherit, "Inherit from the folder"),
            (AuthMode::None, "No auth"),
            (AuthMode::Bearer, "Bearer token"),
            (AuthMode::Basic, "Basic"),
            (AuthMode::Apikey, "API key"),
            (AuthMode::Oauth2, "OAuth 2"),
        ];
        let label_of = |mode: AuthMode| MODES.iter().find(|(each, _)| *each == mode).map_or("Inherit from the folder", |(_, label)| label);
        let edited_path = path.to_owned();
        let mode = ui::select(
            "bruno-auth-mode",
            label_of(editor.auth_mode),
            MODES.iter().map(|(_, label)| Pick::new(*label, *label)).collect(),
            Some(label_of(editor.auth_mode).into()),
            false,
            listen::<Bruno, SharedString>(cx, move |bruno, _, picked, _, cx| {
                if let Some(editor) = bruno.editors.get_mut(&edited_path) {
                    if let Some((mode, _)) = MODES.iter().find(|(_, label)| *label == picked.as_ref()) {
                        editor.auth_mode = *mode;
                    }
                }
                bruno.edited(&edited_path, cx);
                cx.notify();
            }),
            cx,
        );
        let labelled = |label: &'static str, input: &Entity<InputState>| {
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(ui::meta(label, cx))
                .child(ui::Field::new(input).mono())
        };
        let fields = match editor.auth_mode {
            AuthMode::Bearer => Some(div().child(labelled("Token", &editor.token))),
            AuthMode::Basic => Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(labelled("User name", &editor.username))
                    .child(labelled("Password", &editor.password)),
            ),
            AuthMode::Apikey => {
                let edited_path = path.to_owned();
                let placement = ui::select(
                    "bruno-key-placement",
                    match editor.placement {
                        KeyPlacement::Header => "In a header",
                        KeyPlacement::Queryparams => "In the query",
                    },
                    vec![Pick::new("header", "In a header"), Pick::new("query", "In the query")],
                    None,
                    false,
                    listen::<Bruno, SharedString>(cx, move |bruno, _, picked, _, cx| {
                        if let Some(editor) = bruno.editors.get_mut(&edited_path) {
                            editor.placement = match picked.as_ref() {
                                "query" => KeyPlacement::Queryparams,
                                _ => KeyPlacement::Header,
                            };
                        }
                        bruno.edited(&edited_path, cx);
                        cx.notify();
                    }),
                    cx,
                );
                Some(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(labelled("Key", &editor.key))
                        .child(labelled("Value", &editor.key_value))
                        .child(div().flex().child(placement)),
                )
            }
            _ => None,
        };
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(div().flex().child(mode))
            .children(fields)
            .into_any_element()
    }

    fn response(&self, path: &str, cx: &mut Context<PluginPane>) -> AnyElement {
        let t = Tokens::get(cx).clone();
        if self.running.contains(path) {
            return super::empty("Sending…", cx);
        }
        let Some(result) = self.results.get(path).cloned() else {
            return super::empty("No response yet. Press Send.", cx);
        };
        let status = match &result.response {
            Some(response) => {
                let ink = match response.status {
                    200..=299 => t.live,
                    300..=399 => t.accent,
                    400..=499 => t.warn,
                    _ => t.danger,
                };
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        ui::text(format!("{} {}", response.status, response.status_text), 12.5, FontWeight::SEMIBOLD)
                            .text_color(ink),
                    )
                    .child(ui::meta(format!("{} ms", response.duration_ms), cx))
                    .child(ui::meta(bytes(response.size_bytes), cx))
            }
            None => div().child(ui::text("Request failed", 12.5, FontWeight::SEMIBOLD).text_color(t.danger)),
        };
        let failed = result.tests.iter().filter(|check| !check.passed).count();
        let tabs = Segmented::words(
            "bruno-response-tabs",
            RESPONSE_TABS
                .iter()
                .map(|tab| {
                    let count = (*tab == "Tests" && !result.tests.is_empty()).then_some(result.tests.len() as u64);
                    (SharedString::from(*tab), count)
                })
                .collect(),
            self.response_tab,
        )
        .on_select(listen::<Bruno, usize>(cx, |bruno, _, picked, _, cx| {
            bruno.response_tab = *picked;
            cx.notify();
        }));
        let mono = |text: String| {
            div()
                .font_family(theme::MONO)
                .text_size(theme::ui_text(11.5))
                .text_color(t.text_primary)
                .child(text)
        };
        let shown: AnyElement = match RESPONSE_TABS.get(self.response_tab).copied().unwrap_or("Body") {
            "Body" => match (&result.response, &result.error) {
                (Some(response), _) if response.is_binary => ui::meta(format!("A binary body of {}", bytes(response.size_bytes)), cx).into_any_element(),
                (Some(response), _) => mono(pretty(&response.body)).into_any_element(),
                (None, Some(error)) => div().text_color(t.danger).child(error.clone()).into_any_element(),
                (None, None) => ui::meta("Nothing came back.", cx).into_any_element(),
            },
            "Headers" => div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .children(result.response.iter().flat_map(|response| response.headers.iter()).map(|header| {
                    div()
                        .flex()
                        .gap(px(10.))
                        .child(div().w(px(180.)).flex_none().truncate().child(ui::mono(header.name.clone(), 11.5, cx)))
                        .child(div().min_w_0().child(ui::mono(header.value.clone(), 11.5, cx).text_color(t.text_secondary)))
                }))
                .into_any_element(),
            "Timeline" => {
                let mut lines = vec![format!("{} {}", result.sent.method, result.sent.url)];
                lines.extend(result.sent.headers.iter().map(|header| format!("{}: {}", header.name, header.value)));
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(mono(lines.join("\n")))
                    .children((!result.logs.is_empty()).then(|| {
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(heading("Script logs", cx))
                            .children(result.logs.iter().map(|log| {
                                let ink = match log.level.as_str() {
                                    "error" => t.danger,
                                    "warn" => t.warn,
                                    _ => t.text_secondary,
                                };
                                mono(format!("[{}] {}", log.level, log.text)).text_color(ink)
                            }))
                    }))
                    .into_any_element()
            }
            _ if result.tests.is_empty() => ui::meta("No tests ran.", cx).into_any_element(),
            _ => div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(ui::meta(format!("{} passed, {failed} failed", result.tests.len() - failed), cx))
                .children(result.tests.iter().map(|check| {
                    let (icon, ink) = match check.passed {
                        true => (Icon::Check, t.live),
                        false => (Icon::Close, t.danger),
                    };
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(ui::icon(icon, 12.).text_color(ink))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(ui::text(check.name.clone(), 12., FontWeight::NORMAL).text_color(t.text_primary))
                                .children(check.error.clone().map(|error| ui::meta(error, cx))),
                        )
                }))
                .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().px(px(12.)).py(px(10.)).child(status))
            .child(div().px(px(12.)).pb(px(8.)).child(tabs))
            .child(
                div()
                    .id("bruno-response")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(12.))
                    .pb(px(12.))
                    .child(shown),
            )
            .into_any_element()
    }

    fn asking(&self, root: &str, cx: &mut Context<PluginPane>) -> Option<AnyElement> {
        let (path, env) = self.asking.clone()?;
        let (yes, no, root) = (cx.weak_entity(), cx.weak_entity(), root.to_owned());
        Some(
            ui::Confirm::new(
                "Trust this Bruno collection for this session?",
                "Trusted collections may run request scripts, contact localhost and private APIs, and upload files inside the collection folder. Requests still have hard time and size limits.",
                "Trust collection",
            )
            .marked()
            .on_yes(move |_, cx| {
                let (root, path, env) = (root.clone(), path.clone(), env.clone());
                let _ = yes.update(cx, |pane, cx| {
                    if let Some((bruno, host)) = pane.parts::<Bruno>() {
                        bruno.asking = None;
                        bruno.trusted = Some(root.clone());
                        bruno.send(host, &root, &path, env, cx);
                    }
                    cx.notify();
                });
            })
            .on_no(move |_, cx| {
                let _ = no.update(cx, |pane, cx| {
                    if let Some((bruno, _)) = pane.parts::<Bruno>() {
                        bruno.asking = None;
                    }
                    cx.notify();
                });
            })
            .into_any_element(),
        )
    }
}

/// A table of names and values: each row's switch, its two fields and a way to drop it, then
/// a way to add one.
fn table(path: &str, editor: &Editor, table: Table, cx: &mut Context<PluginPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let rows = editor.rows(table).iter().enumerate().map(|(at, row)| {
        let (toggled, dropped) = (path.to_owned(), path.to_owned());
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                gpui_kit::component::checkbox::Checkbox::new(SharedString::from(format!("bruno-row-{table:?}-{at}")))
                    .checked(row.enabled)
                    .on_click(listen::<Bruno, bool>(cx, move |bruno, _, on, _, cx| {
                        if let Some(row) = bruno.editors.get_mut(&toggled).and_then(|editor| editor.rows_mut(table)).and_then(|rows| rows.get_mut(at)) {
                            row.enabled = *on;
                        }
                        bruno.edited(&toggled, cx);
                        cx.notify();
                    })),
            )
            .child(div().flex_1().min_w_0().child(ui::Field::new(&row.name).mono()))
            .child(div().flex_1().min_w_0().child(ui::Field::new(&row.value).mono()))
            .child(
                IconButton::new(SharedString::from(format!("bruno-drop-{table:?}-{at}")), Icon::Close, "Remove")
                    .small()
                    .on_click(listen::<Bruno, _>(cx, move |bruno, _, _, _, cx| {
                        if let Some(rows) = bruno.editors.get_mut(&dropped).and_then(|editor| editor.rows_mut(table)) {
                            if at < rows.len() {
                                rows.remove(at);
                            }
                        }
                        bruno.edited(&dropped, cx);
                        cx.notify();
                    })),
            )
    });
    let added = path.to_owned();
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .children(rows)
        .when(editor.rows(table).is_empty(), |list| list.child(ui::meta("None yet.", cx).text_color(t.text_tertiary)))
        .child(
            div().flex().child(
                Button::new(SharedString::from(format!("bruno-add-{table:?}")), "Add", Variant::Outline)
                    .icon(Icon::Plus)
                    .on_click(listen::<Bruno, _>(cx, move |bruno, _, _, window, cx| {
                        if let Some(editor) = bruno.editors.get_mut(&added) {
                            editor.add_row(table, window, cx);
                        }
                        cx.notify();
                    })),
            ),
        )
        .into_any_element()
}

fn code(editor: &Editor, text: Text, cx: &mut Context<PluginPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let Some(area) = editor.text(text) else {
        return div().into_any_element();
    };
    div()
        .px(px(10.))
        .py(px(8.))
        .rounded(RADIUS_2)
        .border_1()
        .border_color(t.border)
        .bg(t.surface)
        .font_family(theme::MONO)
        .child(Textarea::new(area).appearance(false).text_size(theme::ui_text(12.)))
        .into_any_element()
}

fn heading(text: &'static str, cx: &App) -> Div {
    ui::text(text, 11.5, FontWeight::SEMIBOLD).text_color(Tokens::get(cx).text_secondary)
}

fn label(text: String, t: &Tokens) -> Div {
    ui::text(text, 12., FontWeight::NORMAL)
        .text_color(t.text_primary)
        .flex_1()
        .min_w_0()
        .truncate()
}

fn row(id: String, depth: usize, current: bool, cx: &App) -> Stateful<Div> {
    let t = Tokens::get(cx).clone();
    let hover = t.surface_hover;
    div()
        .id(ElementId::Name(id.into()))
        .h(px(24.))
        .pl(px(6. + depth as f32 * 14.))
        .pr(px(6.))
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded(RADIUS_2)
        .cursor_pointer()
        .when(current, |row| row.bg(t.surface_chosen))
        .when(!current, |row| row.hover(move |row| row.bg(hover)))
}

/// A body as text to read: JSON laid out, cut where it would be too long to draw.
fn pretty(body: &str) -> String {
    let laid = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| body.to_owned());
    match laid.char_indices().nth(BODY_SHOWN) {
        Some((cut, _)) => format!("{}\n…", laid.get(..cut).unwrap_or_default()),
        None => laid,
    }
}

fn bytes(size: u64) -> String {
    match size {
        0..1024 => format!("{size} B"),
        1024..1_048_576 => format!("{:.1} KB", size as f64 / 1024.),
        _ => format!("{:.1} MB", size as f64 / 1_048_576.),
    }
}

/// What the pane says while no collection is open, with the way to open one.
fn welcome(cx: &mut Context<PluginPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(10.))
        .child(ui::icon(Icon::Bruno, 28.).text_color(t.text_tertiary))
        .child(ui::text("Open a Bruno collection", 15., FontWeight::SEMIBOLD).text_color(t.text_primary))
        .child(
            div()
                .max_w(px(420.))
                .text_center()
                .text_size(theme::ui_text(12.5))
                .text_color(t.text_secondary)
                .child("Pick a folder holding a bruno.json, or a workspace of several collections, to send its requests beside your code."),
        )
        .child(
            Button::new("bruno-open", "Open a folder", Variant::Primary)
                .on_click(listen::<Bruno, _>(cx, |_, _, _, _, cx| choose_collection(cx))),
        )
        .into_any_element()
}

/// The environment `id` chosen for the collection `owner`, or none.
fn choose_env(settings: &mut serde_json::Map<String, Value>, owner: String, id: Option<String>) {
    let envs = settings.entry("selectedEnvs").or_insert_with(|| json!({}));
    if let Some(envs) = envs.as_object_mut() {
        match id {
            Some(id) => envs.insert(owner, Value::String(id)),
            None => envs.remove(&owner),
        };
    }
}

/// Opens the collection in `folder`, first among the folders opened before.
fn open_collection(host: &Host, folder: String, cx: &mut App) {
    host.save_settings(
        move |settings| {
            let mut known: Vec<Value> = settings
                .get("workspaces")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            known.retain(|known| known.as_str() != Some(folder.as_str()));
            known.insert(0, Value::String(folder.clone()));
            settings.insert("workspaces".into(), Value::Array(known));
            settings.insert("collectionPath".into(), Value::String(folder));
        },
        cx,
    );
}

/// Asks for a collection folder, and opens it.
fn choose_collection(cx: &mut Context<PluginPane>) {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: None,
    });
    cx.spawn(async move |pane, cx| {
        let Ok(Ok(Some(paths))) = picked.await else {
            return;
        };
        let Some(folder) = paths.first().map(|path| path.to_string_lossy().into_owned()) else {
            return;
        };
        let _ = pane.update(cx, |pane, cx| {
            if let Some((bruno, host)) = pane.parts::<Bruno>() {
                *bruno = Bruno::default();
                open_collection(host, folder, cx);
            }
            cx.notify();
        });
    })
    .detach();
}
