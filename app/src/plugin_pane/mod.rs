//! A plugin's pane. The core reads the views the plugin's manifest names, keeps them and says
//! when one changed; the pane keeps what it was last sent, reads a view again on that word, and
//! hands it to the plugin's own panel to draw. Panels hold only what the person picked; what
//! they show comes from the views, and what they change goes back through the plugin's calls.

use std::any::Any;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::rc::Rc;

use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::notification::Notification;
use gpui_kit::{
    div, px, AnyElement, App, Context, FontWeight, IntoElement, ParentElement, Render,
    SharedString, Styled, Task, WeakEntity, Window,
};
use gpui_transcript::Style;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Map, Value};
use sikemux_core::git::log::GitCommit;

use crate::core::Target;
use crate::icons::Icon;
use crate::send_to::{self, Choice, Recipient};
use crate::theme::{Tokens, SPACE_1H, SPACE_6, TEXT_XS};
use crate::toast;
use crate::ui;
use crate::workbench::Workbench;

/// Every plugin that draws a panel, one line each, sorted: its module declares the rest in
/// its own `REGISTRATION`.
macro_rules! plugins {
    ($($name:ident),* $(,)?) => {
        $(mod $name;)*
        const PLUGINS: &[Registration] = &[$($name::REGISTRATION),*];
    };
}

plugins! {
    aws,
    bruno,
    database,
    jira,
    linear,
    rundeck,
    signoz,
    slack,
    vercel,
}

/// How many views a pane keeps, the least lately asked for going first.
const KEPT_AT_MOST: usize = 32;

/// What a plugin draws in its pane, from the views its host keeps.
pub trait Panel: Any {
    fn render(
        &mut self,
        host: &mut Host,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement;

    /// Shows the place `at` names, as a row picked in one of the plugin's palettes says it.
    fn open_at(&mut self, _at: &str, _host: &mut Host, _cx: &mut Context<PluginPane>) {}

    /// Shows the one thing `at` names alone, as a desk does a link to it, saying whether the
    /// panel can; `at` is what the plugin's `resolveLink` answered.
    fn show_alone(&mut self, _at: &Value, _host: &mut Host, _cx: &mut Context<PluginPane>) -> bool {
        false
    }

    /// What a desk's tab calls the thing shown alone.
    fn alone_title(&self) -> Option<String> {
        None
    }
}

/// The surface of a plugin's item in the window's top bar.
pub const TOP_BAR: &str = "topbar";

/// What a plugin's panel module says of itself: the plugin, the surface its pane opens on,
/// what draws the pane and its top-bar item, and the palettes it offers.
pub struct Registration {
    pub plugin: &'static str,
    pub surface: &'static str,
    pub pane: fn() -> Box<dyn Panel>,
    pub top_bar: Option<fn() -> Box<dyn Panel>>,
    pub palettes: &'static [PluginPalette],
}

/// A panel of type `P`, for a registration to name.
pub fn boxed<P: Panel + Default>() -> Box<dyn Panel> {
    Box::<P>::default()
}

fn registration(plugin: &str) -> Option<&'static Registration> {
    PLUGINS
        .iter()
        .find(|registered| registered.plugin == plugin)
}

/// What `plugin` draws on `surface`: its pane, or its item in the top bar.
fn panel(plugin: &str, surface: &str) -> Option<Box<dyn Panel>> {
    let registered = registration(plugin)?;
    match surface {
        TOP_BAR => registered.top_bar.map(|top_bar| top_bar()),
        _ => Some((registered.pane)()),
    }
}

/// Whether `plugin` draws an item in the window's top bar.
pub fn has_top_bar(plugin: &str) -> bool {
    registration(plugin).is_some_and(|registered| registered.top_bar.is_some())
}

/// A list a plugin offers in the command palette, read from one of its views; picking a row
/// opens the plugin's pane at the place the row's key names.
pub struct PluginPalette {
    pub plugin: &'static str,
    pub id: &'static str,
    /// The command that opens it.
    pub title: &'static str,
    pub placeholder: &'static str,
    pub view: &'static str,
    /// The view's params, from what the plugin's panes saved; none asks with no params.
    pub params: Option<fn(&Value) -> Value>,
    pub rows: fn(&Value) -> Vec<crate::palette::Row>,
    /// A row made of what is typed, such as an id to open, when the words read as one.
    pub typed: Option<fn(&str) -> Option<crate::palette::Row>>,
}

/// The palettes `plugin` offers.
pub fn palettes(plugin: &str) -> &'static [PluginPalette] {
    registration(plugin).map_or(&[], |registered| registered.palettes)
}

/// The surface a plugin's pane opens on, for the plugins that draw one.
pub fn surface(plugin: &str) -> Option<&'static str> {
    registration(plugin).map(|registered| registered.surface)
}

/// The plugin and the surface a pane's target names, as `<plugin>:<surface>`.
pub fn parts(target: &str) -> (&str, &str) {
    target.split_once(':').unwrap_or((target, ""))
}

/// Why a view or a call came to nothing, as the plugin or the core said it.
#[derive(Clone, Debug)]
pub struct Failure {
    pub category: SharedString,
    pub message: SharedString,
    pub status: Option<u64>,
}

impl Failure {
    pub(crate) fn of(failed: &Value) -> Self {
        Self {
            category: failed["category"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
                .into(),
            message: match failed["message"].as_str() {
                Some(message) => message.to_owned().into(),
                None => failed.to_string().into(),
            },
            status: failed["status"].as_u64(),
        }
    }

    pub(crate) fn core(error: String) -> Self {
        Self {
            category: "core".into(),
            message: error.into(),
            status: None,
        }
    }

    /// The same failure, said as what was being done when it happened.
    pub fn titled(&self, doing: &str) -> Self {
        Self {
            message: format!("{doing}: {}", self.message).into(),
            ..self.clone()
        }
    }

    /// The plugin is not signed in, or its sign-in no longer works.
    pub fn signed_out(&self) -> bool {
        matches!(self.category.as_ref(), "auth" | "unconfigured")
            || (self.category == "http" && self.status == Some(401))
    }
}

/// What a plugin's stream hands its panel: an item, or the end, with why it failed if it did.
pub enum Streamed {
    Item(Value),
    Ended(Option<Failure>),
}

/// A view as the pane has it now.
pub enum View<'a> {
    Loading,
    Ready(&'a Value, &'a Read),
    Failed(&'a Failure),
}

/// A view as a panel last read it, so it is read once each time it arrives.
pub type Read = RefCell<Option<Rc<dyn Any>>>;

impl View<'_> {
    /// The view read as `T`; a view that does not read as one counts as failed.
    pub fn read<T: DeserializeOwned + 'static>(&self) -> Result<Option<Rc<T>>, Failure> {
        match self {
            View::Loading => Ok(None),
            View::Ready(value, read) => {
                if let Some(known) = read
                    .borrow()
                    .clone()
                    .and_then(|known| known.downcast().ok())
                {
                    return Ok(Some(known));
                }
                let fresh: Rc<T> = serde_json::from_value::<T>((*value).clone())
                    .map(Rc::new)
                    .map_err(|error| Failure::core(format!("unreadable view: {error}")))?;
                *read.borrow_mut() = Some(fresh.clone());
                Ok(Some(fresh))
            }
            View::Failed(failure) => Err((*failure).clone()),
        }
    }
}

enum Answer {
    Waiting,
    Got(Value, Read),
    Failed(Failure),
}

struct Kept {
    answer: Answer,
    asked: u64,
    _reading: Task<()>,
}

/// What a panel asks its plugin through: the views it shows and the calls it makes.
pub struct Host {
    pub plugin: String,
    pub surface: String,
    /// Where in the window the pane is, so two panes of one plugin each keep their views.
    slot: String,
    target: Target,
    bench: WeakEntity<Workbench>,
    kept: HashMap<(String, String), Kept>,
    asks: u64,
    status: Option<(Option<bool>, Kept)>,
    /// The browser sign-in on its way, which dropping stops.
    signing: Option<Task<()>>,
    pub faces: crate::avatars::Avatars,
    /// The pane's tab is out of front, so its views are not read again until it shows.
    hidden: bool,
}

/// Opens a sign-in page in `browser`, the app and the shortcut that brings it forward, else in
/// the default browser.
fn open_sign_in(url: String, browser: Option<(String, String)>, cx: &mut gpui::AsyncApp) {
    #[cfg(target_os = "macos")]
    if let Some((app, shortcut)) = browser {
        cx.background_executor()
            .spawn(async move { crate::cloud_browser::open(&url, &app, &shortcut) })
            .detach();
        return;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = browser;
    cx.update(|cx| cx.open_url(&url));
}

impl Host {
    /// The plugin's view `method` with `params`, asked of the core the first time and kept
    /// after, until the core says it changed.
    pub fn view(
        &mut self,
        method: &str,
        params: impl Serialize,
        cx: &mut Context<PluginPane>,
    ) -> View<'_> {
        let params = to_params(params);
        self.asks += 1;
        let key = (method.to_owned(), params.to_string());
        if !self.kept.contains_key(&key) {
            self.forget_oldest();
            let reading = self.read(method, params, cx);
            self.kept.insert(
                key.clone(),
                Kept {
                    answer: Answer::Waiting,
                    asked: self.asks,
                    _reading: reading,
                },
            );
        }
        let kept = self.kept.get_mut(&key).expect("kept just now");
        kept.asked = self.asks;
        match &kept.answer {
            Answer::Waiting => View::Loading,
            Answer::Got(value, read) => View::Ready(value, read),
            Answer::Failed(failure) => View::Failed(failure),
        }
    }

    fn forget_oldest(&mut self) {
        if self.kept.len() < KEPT_AT_MOST {
            return;
        }
        let oldest = self
            .kept
            .iter()
            .min_by_key(|(_, kept)| kept.asked)
            .map(|(key, _)| key.clone());
        if let Some(oldest) = oldest {
            self.kept.remove(&oldest);
        }
    }

    fn read(&self, method: &str, params: Value, cx: &mut Context<PluginPane>) -> Task<()> {
        let key = (method.to_owned(), params.to_string());
        let asked = self.target.request(
            "pluginView",
            json!({
                "plugin": self.plugin,
                "method": method,
                "params": params,
                "slot": format!("{}/{method}", self.slot),
            }),
        );
        cx.spawn(async move |pane, cx| {
            let answer = match asked.await {
                Ok(answer) => match answer.get("failed") {
                    Some(failed) => Answer::Failed(Failure::of(failed)),
                    None => Answer::Got(answer["value"].clone(), Read::default()),
                },
                Err(error) => Answer::Failed(Failure::core(error)),
            };
            let _ = pane.update(cx, |pane, cx| {
                if let Some(kept) = pane.host.kept.get_mut(&key) {
                    kept.answer = answer;
                    cx.notify();
                }
            });
        })
    }

    /// Reads again the kept views the core says changed, `paths` naming them as
    /// `<plugin>/<method>`, or every one when it names none.
    fn changed(&mut self, paths: Option<&[String]>, cx: &mut Context<PluginPane>) {
        if self.hidden {
            return;
        }
        let moved = |method: &str| {
            paths.is_none_or(|paths| {
                paths
                    .iter()
                    .any(|path| *path == format!("{}/{method}", self.plugin))
            })
        };
        let stale: Vec<(String, String)> = self
            .kept
            .keys()
            .filter(|(method, _)| moved(method))
            .cloned()
            .collect();
        for key in stale {
            let params = serde_json::from_str(&key.1).unwrap_or(Value::Null);
            let reading = self.read(&key.0, params, cx);
            if let Some(kept) = self.kept.get_mut(&key) {
                kept._reading = reading;
            }
        }
    }

    /// Shows `value` as the view `method` with `params`, read straight from the plugin, until
    /// the core next says it changed.
    pub fn put_view(&mut self, method: &str, params: &Value, value: Value) {
        if let Some(kept) = self.kept.get_mut(&(method.to_owned(), params.to_string())) {
            kept.answer = Answer::Got(value, Read::default());
        }
    }

    /// Calls the plugin's `method`; the core reads its views again after a write.
    pub fn call(
        &self,
        method: &str,
        params: impl Serialize,
    ) -> impl Future<Output = Result<Value, Failure>> + 'static {
        let asked = self.target.request(
            "plugin",
            json!({ "plugin": self.plugin, "method": method, "params": to_params(params) }),
        );
        async move {
            let answer = asked.await.map_err(Failure::core)?;
            match answer.get("failed") {
                Some(failed) => Err(Failure::of(failed)),
                None => Ok(answer["value"].clone()),
            }
        }
    }

    /// Follows the plugin's streamed `method`, handing panel `P` each item as it arrives and
    /// then how the stream ended. Dropping the task stops the stream.
    pub fn stream<P: Panel>(
        &self,
        method: &str,
        params: impl Serialize,
        cx: &mut Context<PluginPane>,
        each: impl Fn(&mut P, &mut Host, Streamed, &mut Context<PluginPane>) + 'static,
    ) -> Task<()> {
        let mut streaming = self.target.stream(
            "pluginStream",
            json!({ "plugin": self.plugin, "method": method, "params": to_params(params) }),
        );
        cx.spawn(async move |pane, cx| loop {
            let part = streaming.parts.recv().await;
            let ended = part
                .as_ref()
                .is_none_or(|part| part.done || part.error.is_some());
            let gone = pane
                .update(cx, |pane, cx| {
                    let Some((panel, host)) = pane.parts::<P>() else {
                        return;
                    };
                    let failure = part.as_ref().and_then(|part| part.error.as_ref());
                    let failure = failure.map(Failure::of);
                    for item in part.into_iter().flat_map(|part| part.items) {
                        each(panel, host, Streamed::Item(item), cx);
                    }
                    if ended {
                        each(panel, host, Streamed::Ended(failure), cx);
                    }
                    cx.notify();
                })
                .is_err();
            if ended || gone {
                break;
            }
        })
    }

    fn ask_core(
        &self,
        method: &'static str,
        params: Value,
    ) -> impl Future<Output = Result<Value, Failure>> + 'static {
        let asked = self.target.request(method, params);
        async move { asked.await.map_err(Failure::core) }
    }

    /// Checks out `branch` in the repository at `repo`, carrying uncommitted work along.
    pub fn checkout_branch(
        &self,
        repo: &str,
        branch: &str,
    ) -> impl Future<Output = Result<Value, Failure>> + 'static {
        self.ask_core(
            "git.checkoutSmart",
            json!({ "repo": repo, "branch": branch }),
        )
    }

    /// The repository the Git views follow, in the project in front.
    pub fn git_repo(&self, cx: &App) -> Option<String> {
        self.bench.upgrade()?.read(cx).git_repo()
    }

    /// The recent commits of the repository at `repo` that name `key`, as the core finds them.
    pub fn commits_naming(
        &self,
        repo: &str,
        key: &str,
    ) -> impl Future<Output = Result<Value, Failure>> + 'static {
        self.ask_core("git.commitsNaming", json!({ "repo": repo, "key": key }))
    }

    /// Pushes the branch checked out in the repository at `repo`.
    pub fn push(&self, repo: &str) -> impl Future<Output = Result<Value, Failure>> + 'static {
        self.ask_core("git.push", json!({ "repo": repo }))
    }

    /// What the plugin's `status` call says of its sign-in, such as its accounts or sites,
    /// asked again whenever the core lists the plugin as signed in or out.
    pub fn status(&mut self, cx: &mut Context<PluginPane>) -> View<'_> {
        let signed_in = self.signed_in(cx);
        if self
            .status
            .as_ref()
            .is_none_or(|(asked, _)| *asked != signed_in)
        {
            let call = self.call("status", Value::Null);
            let reading = cx.spawn(async move |pane, cx| {
                let answer = match call.await {
                    Ok(value) => Answer::Got(value, Read::default()),
                    Err(failure) => Answer::Failed(failure),
                };
                let _ = pane.update(cx, |pane, cx| {
                    if let Some((_, kept)) = &mut pane.host.status {
                        kept.answer = answer;
                        cx.notify();
                    }
                });
            });
            let kept = Kept {
                answer: Answer::Waiting,
                asked: 0,
                _reading: reading,
            };
            self.status = Some((signed_in, kept));
        }
        match self.status.as_ref().map(|(_, kept)| &kept.answer) {
            Some(Answer::Got(value, read)) => View::Ready(value, read),
            Some(Answer::Failed(failure)) => View::Failed(failure),
            _ => View::Loading,
        }
    }

    /// What the plugin's panes saved with the workspace, under its id.
    pub fn settings(&self, cx: &App) -> Value {
        self.bench
            .upgrade()
            .and_then(|bench| {
                bench
                    .read(cx)
                    .preferences()
                    .plugin_settings
                    .get(&self.plugin)
                    .cloned()
            })
            .unwrap_or(Value::Null)
    }

    /// What the plugin's panes saved, read as `T`; a field that does not read keeps its default
    /// and leaves the others as they were saved.
    pub fn read_settings<T: Serialize + DeserializeOwned + Default>(&self, cx: &App) -> T {
        let saved = self.settings(cx);
        if saved.is_null() {
            return T::default();
        }
        if let Ok(read) = T::deserialize(&saved) {
            return read;
        }
        let Value::Object(saved) = saved else {
            say_once(format!(
                "sikemux: {}'s saved settings are not an object, so they are left at their defaults",
                self.plugin
            ));
            return T::default();
        };
        let mut read = Map::new();
        for (key, value) in saved {
            let mut tried = read.clone();
            tried.insert(key.clone(), value);
            match T::deserialize(&Value::Object(tried.clone())) {
                Ok(_) => read = tried,
                Err(error) => say_once(format!(
                    "sikemux: {}'s saved {key} did not read, so it is left at its default: {error}",
                    self.plugin
                )),
            }
        }
        T::deserialize(&Value::Object(read)).unwrap_or_default()
    }

    /// Changes the plugin's saved settings as `change` does to them read as `T`, saving only
    /// the fields it changed.
    pub fn update_settings<T: Serialize + DeserializeOwned + Default>(
        &self,
        change: impl FnOnce(&mut T),
        cx: &mut App,
    ) {
        let mut settings = self.read_settings::<T>(cx);
        let before = fields(&settings);
        change(&mut settings);
        let after = fields(&settings);
        self.save_settings(
            |kept| {
                for (key, value) in after {
                    if before.get(&key) != Some(&value) {
                        kept.insert(key, value);
                    }
                }
            },
            cx,
        );
    }

    /// Changes the plugin's saved settings as `change` does to their JSON.
    pub fn save_settings(&self, change: impl FnOnce(&mut Map<String, Value>), cx: &mut App) {
        let Some(bench) = self.bench.upgrade() else {
            return;
        };
        let mut all = bench.read(cx).preferences().plugin_settings.clone();
        let mut kept = all.remove(&self.plugin).unwrap_or_else(|| json!({}));
        if !kept.is_object() {
            kept = json!({});
        }
        if let Some(kept) = kept.as_object_mut() {
            change(kept);
        }
        all.insert(self.plugin.clone(), kept);
        bench.update(cx, |bench, _| {
            bench.call(
                "setPreference",
                json!({ "key": "pluginSettings", "value": all }),
            )
        });
    }

    /// Someone's picture, fetched through the plugin's `image` call, or their initials.
    pub fn face(&self, name: &str, url: Option<&str>, size: f32, cx: &App) -> AnyElement {
        self.faces.face(name, url, size, cx)
    }

    /// Copies `text` and says what was copied.
    pub fn copy(&self, text: String, what: &str, cx: &mut App) {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
        self.notify(toast::success(format!("Copied {what}")), cx);
    }

    /// Says `note` over the window.
    pub fn notify(&self, note: Notification, cx: &mut App) {
        if let Some(bench) = self.bench.upgrade() {
            bench.update(cx, |bench, cx| bench.notify(note, cx));
        }
    }

    /// Asks the person over the window, handing `then` whether they said `yes`.
    pub fn confirm(
        &self,
        title: impl Into<SharedString>,
        body: impl Into<SharedString>,
        yes: impl Into<SharedString>,
        destructive: bool,
        then: impl Fn(bool, &mut App) + 'static,
        cx: &mut App,
    ) {
        if let Some(bench) = self.bench.upgrade() {
            bench.update(cx, |bench, cx| {
                bench.confirm(
                    title,
                    body,
                    yes,
                    destructive,
                    move |_, said, _, cx| then(said, cx),
                    cx,
                )
            });
        }
    }

    /// How the pane's markdown reads at `text_size`, its links opening as the window opens them.
    pub fn reading(&self, text_size: f32, cx: &App) -> Style {
        crate::editor::preview::reading_linked(text_size, &self.bench, cx)
    }

    /// The project folder the Git views follow.
    pub fn watched_project(&self, cx: &App) -> Option<String> {
        let bench = self.bench.upgrade()?;
        bench.read(cx).watched_project.clone()
    }

    /// The branch checked out in the project in front, as the Git views last read it: none
    /// until they have, and a read without a branch while HEAD is detached.
    pub fn current_branch(&self, cx: &App) -> Option<Option<String>> {
        let bench = self.bench.upgrade()?;
        let overview = bench.read(cx).reads.git_overview.as_ref()?.as_ref().ok()?;
        let branch = &overview.status.branch;
        Some((!branch.is_empty()).then(|| branch.clone()))
    }

    /// The latest commits in the project in front, as the Git views last read them.
    pub fn recent_commits(&self, cx: &App) -> Option<Vec<GitCommit>> {
        let bench = self.bench.upgrade()?;
        let overview = bench.read(cx).reads.git_overview.as_ref()?.as_ref().ok()?;
        Some(overview.log.clone())
    }

    /// Puts a ticket on the board of the project in front, as the core's `boardAddTicket` takes
    /// it, and says over the window what came of it, `put` once it is on.
    pub fn add_board_ticket(&self, params: Value, put: String, cx: &mut App) {
        let Some(bench) = self.bench.upgrade() else {
            return;
        };
        let asked = bench.read(cx).target().request("boardAddTicket", params);
        let weak = bench.downgrade();
        cx.spawn(async move |cx| {
            let answer = asked.await;
            let _ = weak.update(cx, |bench, cx| match answer {
                Ok(board) => {
                    for problem in board["problems"].as_array().into_iter().flatten() {
                        if let Some(problem) = problem.as_str() {
                            bench.notify(toast::error(problem.to_owned()), cx);
                        }
                    }
                    bench.notify(toast::success(put), cx);
                }
                Err(error) => {
                    let failed = format!("Send to the board: {error}");
                    bench.notify(toast::error(failed), cx)
                }
            });
        })
        .detach();
    }

    /// Has the core read the plugin's views again now, each of `method`'s or all of them.
    pub fn read_again(&self, method: Option<&str>) {
        self.target.call(
            "pluginViewsRead",
            json!({ "plugin": self.plugin, "method": method }),
        );
    }

    /// The project in front, by its path.
    pub fn project(&self, cx: &App) -> Option<String> {
        self.front_project(cx).map(|(path, _)| path)
    }

    /// The project in front, by its path and its name.
    pub fn front_project(&self, cx: &App) -> Option<(String, String)> {
        let bench = self.bench.upgrade()?;
        let project = bench.read(cx).project()?;
        Some((
            project.path.to_string_lossy().into_owned(),
            project.name.clone(),
        ))
    }

    /// Whether the pointer is over the window's top bar.
    pub fn hovered(&self, cx: &App) -> bool {
        self.bench
            .upgrade()
            .is_some_and(|bench| bench.read(cx).top_bar_hovered)
    }

    /// Opens the plugin's pane, or brings it its next arrival, at the place `at` names.
    pub fn open_pane_at(&self, at: String, cx: &mut App) {
        let plugin = self.plugin.clone();
        if let Some(bench) = self.bench.upgrade() {
            bench.update(cx, |bench, cx| bench.open_plugin_at(&plugin, at, cx));
        }
    }

    /// The host whose core the pane asks.
    pub fn place(&self) -> &crate::hosts::Place {
        self.target.place()
    }

    /// Whether the person is signed in to the plugin, as the core last listed it.
    pub fn signed_in(&self, cx: &App) -> Option<bool> {
        let bench = self.bench.upgrade()?;
        bench.read(cx).reads.plugins.signed_in(&self.plugin)
    }

    /// Signs in through the plugin's own browser sign-in, opening the page it names.
    pub fn sign_in_with_browser(&mut self, cx: &mut Context<PluginPane>) {
        self.stream_sign_in("signInWithBrowser", json!({}), None, cx);
    }

    /// Signs in to a cloud through the plugin's streamed `method`, opening its pages in the
    /// browser Settings › Single sign-on names for single sign-on, else the default one.
    pub fn cloud_sign_in(&mut self, method: &str, params: Value, cx: &mut Context<PluginPane>) {
        let browser = self.bench.upgrade().and_then(|bench| {
            let preferences = bench.read(cx).preferences();
            let app = preferences.cloud_browser.trim().to_owned();
            (!app.is_empty()).then_some((app, preferences.cloud_browser_shortcut.clone()))
        });
        self.stream_sign_in(method, params, browser, cx);
    }

    /// Whether a browser sign-in is on its way.
    pub fn signing_in(&self) -> bool {
        self.signing.is_some()
    }

    pub fn cancel_sign_in(&mut self) {
        self.signing = None;
    }

    fn stream_sign_in(
        &mut self,
        method: &str,
        params: Value,
        browser: Option<(String, String)>,
        cx: &mut Context<PluginPane>,
    ) {
        let mut streaming = self.target.stream(
            "pluginStream",
            json!({ "plugin": self.plugin, "method": method, "params": params }),
        );
        let bench = self.bench.clone();
        self.signing = Some(cx.spawn(async move |pane, cx| {
            let mut failed = None;
            while let Some(part) = streaming.parts.recv().await {
                for item in part.items {
                    if let Some(url) = item["url"].as_str() {
                        open_sign_in(url.to_owned(), browser.clone(), cx);
                    }
                }
                if let Some(error) = part.error {
                    failed = Some(Failure::of(&error));
                }
                if part.done {
                    break;
                }
            }
            let _ = pane.update(cx, |pane, cx| {
                pane.host.signing = None;
                cx.notify();
            });
            if let Some(failed) = failed {
                let _ = bench.update(cx, |bench, cx| {
                    let note = toast::error(format!("Sign-in did not finish: {}", failed.message));
                    bench.notify(note, cx);
                });
            }
        }));
        cx.notify();
    }

    /// The agents the pane can hand text to.
    pub fn agents(&self, cx: &App) -> Vec<Choice> {
        self.bench
            .upgrade()
            .map_or_else(Vec::new, |bench| send_to::choices(bench.read(cx)))
    }

    /// Puts `text` in the composer of the agent `to` names and brings it forward.
    pub fn send_to_agent(&self, to: Recipient, text: String, window: &mut Window, cx: &mut App) {
        if let Some(bench) = self.bench.upgrade() {
            bench.update(cx, |bench, cx| {
                send_to::send(bench, to, async move { Some(text) }, window, cx)
            });
        }
    }

    /// Says what went wrong with a call, over the window.
    pub fn said(&self, failure: &Failure, cx: &mut App) {
        self.notify(toast::error(failure.message.clone()), cx);
    }
}

/// A plugin's pane: its host and the panel that draws it.
pub struct PluginPane {
    pub host: Host,
    panel: Option<Box<dyn Panel>>,
    /// Shows one thing alone on a desk, in a slot of its own, so the core stops reading its
    /// views once the desk lets it go.
    alone: bool,
}

impl Drop for PluginPane {
    fn drop(&mut self) {
        if self.alone {
            self.hide();
        }
    }
}

impl PluginPane {
    pub fn new(target: &str, slot: String, core: Target, bench: WeakEntity<Workbench>) -> Self {
        let (plugin, surface) = parts(target);
        Self {
            panel: panel(plugin, surface),
            alone: false,
            host: Host {
                plugin: plugin.to_owned(),
                surface: surface.to_owned(),
                slot,
                target: core,
                bench,
                kept: HashMap::new(),
                asks: 0,
                status: None,
                signing: None,
                faces: Default::default(),
                hidden: false,
            },
        }
    }

    /// Stops the core reading the pane's views again while its tab is out of front.
    pub fn hide(&mut self) {
        if !std::mem::replace(&mut self.host.hidden, true) {
            self.host
                .target
                .call("pluginViewAway", json!({ "slot": self.host.slot }));
        }
    }

    /// Reads the pane's views again as its tab comes to the front, which the core then keeps
    /// reading while it shows.
    pub fn show(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.host.hidden) {
            self.host.changed(None, cx);
        }
    }

    /// The pane's panel as the `P` it is, with the host beside it.
    pub fn parts<P: Panel>(&mut self) -> Option<(&mut P, &mut Host)> {
        let panel: &mut dyn Any = self.panel.as_deref_mut()?;
        Some((panel.downcast_mut::<P>()?, &mut self.host))
    }

    /// Shows the one thing `at` names alone, as a desk does a link to it, saying whether the
    /// plugin's panel can.
    pub fn show_alone(&mut self, at: &Value, cx: &mut Context<Self>) -> bool {
        self.alone = self
            .panel
            .as_mut()
            .is_some_and(|panel| panel.show_alone(at, &mut self.host, cx));
        cx.notify();
        self.alone
    }

    /// How a desk's tab names the thing shown alone: the plugin's mark and its title.
    pub fn desk_face(&self, cx: &App) -> (AnyElement, String) {
        let name = self.host.plugin.trim_start_matches("sikemux.");
        let glyph = crate::rail::plugin_mark(&self.host.plugin).unwrap_or(Icon::Window);
        let mark = ui::picture(glyph, 13., Tokens::get(cx).text_secondary);
        let title = self.panel.as_ref().and_then(|panel| panel.alone_title());
        (mark, title.unwrap_or_else(|| name.to_owned()))
    }

    /// Shows the place `at` names, as a palette row picked for this plugin says it.
    pub fn open_at(&mut self, at: &str, cx: &mut Context<Self>) {
        if let Some(panel) = &mut self.panel {
            panel.open_at(at, &mut self.host, cx);
        }
        cx.notify();
    }

    /// The core says some of the plugin's views changed.
    pub fn changed(&mut self, paths: Option<&[String]>, cx: &mut Context<Self>) {
        self.host.changed(paths, cx);
    }
}

impl Render for PluginPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let drawn = match &mut self.panel {
            Some(panel) => panel.render(&mut self.host, window, cx),
            None => empty("This plugin draws no pane here yet.", cx),
        };
        let host = &self.host;
        host.faces.fetch(
            |url| {
                let fetched = host.call("image", json!({ "url": url }));
                async move { fetched.await.ok()?.as_str().map(str::to_owned) }
            },
            |pane| &pane.host.faces,
            cx,
        );
        match self.host.surface.as_str() {
            TOP_BAR => div().flex().items_center().child(drawn),
            _ => div().size_full().flex().flex_col().child(drawn),
        }
    }
}

fn fields(settings: &impl Serialize) -> Map<String, Value> {
    match serde_json::to_value(settings) {
        Ok(Value::Object(fields)) => fields,
        _ => Map::new(),
    }
}

/// A plugin call's params as the JSON the core passes on.
fn to_params(params: impl Serialize) -> Value {
    serde_json::to_value(params).unwrap_or_default()
}

/// Logs `said` the first time only, for what is read again on every frame.
fn say_once(said: String) {
    thread_local! {
        static SAID: RefCell<HashSet<String>> = RefCell::default();
    }
    if SAID.with(|kept| kept.borrow_mut().insert(said.clone())) {
        eprintln!("{said}");
    }
}

/// A handler on panel `P` and its host, for an element of the panel's.
pub fn listen<P: Panel, E: ?Sized>(
    cx: &Context<PluginPane>,
    act: impl Fn(&mut P, &mut Host, &E, &mut Window, &mut Context<PluginPane>) + 'static,
) -> impl Fn(&E, &mut Window, &mut App) + 'static {
    let pane = cx.weak_entity();
    move |event, window, cx| {
        let _ = pane.update(cx, |pane, cx| {
            if let Some((panel, host)) = pane.parts::<P>() {
                act(panel, host, event, window, cx);
            }
        });
    }
}

/// Runs `work`, then hands what it gave to panel `P`.
pub fn after<P: Panel, T: 'static>(
    work: impl Future<Output = T> + 'static,
    then: impl FnOnce(&mut P, &mut Host, T, &mut Context<PluginPane>) + 'static,
    cx: &mut Context<PluginPane>,
) {
    cx.spawn(async move |pane, cx| {
        let done = work.await;
        let _ = pane.update(cx, |pane, cx| {
            if let Some((panel, host)) = pane.parts::<P>() {
                then(panel, host, done, cx);
                cx.notify();
            }
        });
    })
    .detach();
}

/// Said while a view is still on its way.
pub fn loading(cx: &App) -> AnyElement {
    notice(None, "Loading…", None, cx)
}

/// Said when a list or a page has nothing to show.
pub fn empty(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    notice(None, text, None, cx)
}

/// What went wrong with a view.
pub fn failure(failure: &Failure, cx: &App) -> AnyElement {
    notice(
        Some(Icon::Warning),
        "This did not load",
        Some(failure.message.clone()),
        cx,
    )
}

/// Asks the person to sign in to the plugin through its browser sign-in.
pub fn signed_out(
    host: &Host,
    mark: Icon,
    name: &'static str,
    detail: &'static str,
    cx: &mut Context<PluginPane>,
) -> AnyElement {
    let ink = Tokens::get(cx).text_primary;
    ui::sign_in::Screen::new(
        "plugin-sign-in-page",
        ui::picture(mark, 26., ink),
        format!("Sign in to {name}"),
    )
    .lede(detail)
    .foot(ui::sign_in::KEPT)
    .child(browser_sign_in(
        host,
        "plugin-sign-in",
        Icon::LogIn,
        "Sign in with the browser",
        |host, cx| host.sign_in_with_browser(cx),
        cx,
    ))
    .into_any_element()
}

/// The button that starts the plugin's browser sign-in, or, while it runs, the row that waits
/// for it with the way to stop it.
pub fn browser_sign_in(
    host: &Host,
    id: &'static str,
    mark: Icon,
    label: impl Into<SharedString>,
    start: impl Fn(&mut Host, &mut Context<PluginPane>) + 'static,
    cx: &mut Context<PluginPane>,
) -> AnyElement {
    if host.signing_in() {
        return ui::sign_in::waiting(
            id,
            "Finish signing in in your browser",
            cx.listener(|pane, _, _, cx| {
                pane.host.cancel_sign_in();
                cx.notify();
            }),
            cx,
        )
        .into_any_element();
    }
    ui::Button::new(id, label, ui::Variant::Primary)
        .wide()
        .icon(mark)
        .on_click(cx.listener(move |pane, _, _, cx| start(&mut pane.host, cx)))
        .into_any_element()
}

/// A menu's way to hand `text` to an agent, for the pane `pane` names.
pub fn send_submenu(
    popup: PopupMenu,
    label: impl Into<SharedString>,
    pane: &WeakEntity<PluginPane>,
    text: String,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(bench) = pane.upgrade().map(|pane| pane.read(cx).host.bench.clone()) else {
        return popup;
    };
    send_to::submenu_with_text(popup, label, bench, text, window, cx)
}

/// Hands `text` to the agent the person picks, or a new chat, as its next message.
pub fn send_to_agent(
    id: &'static str,
    host: &Host,
    text: String,
    cx: &mut Context<PluginPane>,
) -> AnyElement {
    let agents = Rc::new(host.agents(cx));
    let picks = agents
        .iter()
        .enumerate()
        .map(|(at, agent)| ui::Pick::new(at.to_string(), agent.label.clone()))
        .collect();
    let pane = cx.entity();
    ui::select(
        id,
        "Send to agent",
        picks,
        None,
        agents.is_empty(),
        move |key, window, cx| {
            let Some(agent) = key.parse::<usize>().ok().and_then(|at| agents.get(at)) else {
                return;
            };
            let (to, text) = (agent.to.clone(), text.clone());
            pane.update(cx, |pane, cx| pane.host.send_to_agent(to, text, window, cx));
        },
        cx,
    )
}

fn notice(
    icon: Option<Icon>,
    title: impl Into<SharedString>,
    detail: Option<SharedString>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(SPACE_1H)
        .p(SPACE_6)
        .children(icon.map(|icon| ui::icon(icon, 16.).text_color(t.text_tertiary)))
        .child(ui::text(title, 12.5, FontWeight::MEDIUM).text_color(t.text_secondary))
        .children(detail.map(|detail| {
            div()
                .max_w(px(420.))
                .text_center()
                .text_size(crate::theme::ui_text(TEXT_XS))
                .text_color(t.text_tertiary)
                .child(detail)
        }))
        .into_any_element()
}
