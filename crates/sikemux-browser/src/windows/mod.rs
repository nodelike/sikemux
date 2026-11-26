//! A tab's page on Windows: Edge's WebView2, the Evergreen runtime the system keeps up to
//! date, in a child window of ours inside the app's window. Pages live on the window's
//! thread. A page is made in the background, so opening a tab never holds the window; what a
//! tab is asked to do meanwhile waits and runs once its page is there.
//!
//! Each page sits in a clip window as wide as the part of the page that shows, so a page
//! reaching past the stage's edge is cut there.

pub(crate) mod dialogs;
pub(crate) mod input;
pub(crate) mod page;
mod ui;

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use sikemux_process::sync::Locked;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{
    take_pwstr, AddScriptToExecuteOnDocumentCreatedCompletedHandler,
    CallDevToolsProtocolMethodCompletedHandler, CoreWebView2EnvironmentOptions,
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    DocumentTitleChangedEventHandler, DownloadStartingEventHandler, ExecuteScriptCompletedHandler,
    NavigationCompletedEventHandler, NavigationStartingEventHandler,
    NewWindowRequestedEventHandler, ProcessFailedEventHandler, SourceChangedEventHandler,
    StateChangedEventHandler, WindowCloseRequestedEventHandler,
};
use windows::core::{w, Interface, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, RECT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SetWindowPos, ShowWindow,
    HWND_BOTTOM, SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE, SW_SHOWNA, WINDOW_EX_STYLE, WNDCLASSW,
    WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
};

use crate::tab::{ParentView, ProfileStore, TabSpec};
use crate::{DocumentEvent, PageLoad, TabEvents, TabStall, PARKED_ORIGIN};
pub use ui::run_pages_on_this_thread;
use ui::{on_ui, OnUi};

pub const NOT_HERE: &str = "the browser does not do that on Windows yet";
/// Said in the tab when the system has no WebView2 to draw pages with.
pub const NO_RUNTIME: &str =
    "the browser needs Microsoft Edge WebView2, which is not installed on this computer";
const NO_THREAD: &str = "the window has not started the browser";
const BROWSER_ARGUMENTS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection";

/// The installed runtime's version, or why there is none.
pub fn runtime_version() -> Result<String, String> {
    let mut version = PWSTR::null();
    // SAFETY: the loader fills `version` with a string it allocated, which `take_pwstr` frees.
    unsafe { GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut version) }
        .map_err(|_| NO_RUNTIME.to_string())?;
    let version = take_pwstr(version);
    if version.is_empty() {
        return Err(NO_RUNTIME.into());
    }
    Ok(version)
}

#[derive(Clone)]
pub struct Tab {
    id: Arc<str>,
    held: Arc<Held>,
}

type Act = Box<dyn FnOnce(&Page) + Send>;

enum State {
    Opening(Vec<Act>),
    Open(Page),
    Failed(String),
    Closed,
}

/// The tab's page, or what waits for it, closed on the window's thread whichever thread lets
/// go last.
struct Held(Mutex<State>);

// SAFETY: the page's COM objects inside are only reached on the window's thread: `with`
// sends every act there, and `Drop` sends the page there to close.
unsafe impl Send for Held {}
// SAFETY: a shared `Held` reaches its page only through its mutex, and uses the page itself
// only on the window's thread, for the same reasons as `Send` above.
unsafe impl Sync for Held {}

impl Held {
    /// Runs `act` on the page on the window's thread, once it is there and unless it closed.
    fn with(self: &Arc<Self>, act: impl FnOnce(&Page) + Send + 'static) {
        let held = self.clone();
        on_ui(move || {
            let page = match &mut *held.0.locked() {
                State::Opening(waiting) => return waiting.push(Box::new(act)),
                State::Open(page) => page.clone(),
                State::Failed(_) | State::Closed => return,
            };
            act(&page);
        });
    }

    fn opened(&self, page: Page) {
        let waiting = {
            let mut state = self.0.locked();
            match std::mem::replace(&mut *state, State::Open(page.clone())) {
                State::Opening(waiting) => waiting,
                closed => {
                    *state = closed;
                    drop(state);
                    return page.close();
                }
            }
        };
        for act in waiting {
            act(&page);
        }
    }

    fn failed(&self, why: String) {
        eprintln!("sikemux: a browser tab could not open: {why}");
        let mut state = self.0.locked();
        if matches!(*state, State::Opening(_)) {
            *state = State::Failed(why);
        }
    }

    fn failure(&self) -> Option<String> {
        match &*self.0.locked() {
            State::Failed(why) => Some(why.clone()),
            _ => None,
        }
    }

    fn close(&self) {
        if let State::Open(page) = std::mem::replace(&mut *self.0.locked(), State::Closed) {
            page.close();
        }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if let State::Open(page) = std::mem::replace(&mut *self.0.locked(), State::Closed) {
            let page = OnUi(page);
            on_ui(move || {
                page.into_inner().close();
            });
        }
    }
}

/// A tab's page: its WebView2 and the clip window it sits in.
#[derive(Clone)]
pub struct Page {
    pub(crate) tab_id: String,
    pub(crate) controller: ICoreWebView2Controller,
    pub(crate) webview: ICoreWebView2,
    clip: Clip,
    /// The page's helper world, and the document it was made for.
    pub(crate) helper: Arc<Mutex<Option<(String, i64)>>>,
    pub(crate) pending_evals: Arc<page::PendingEvals>,
}

impl Page {
    fn close(&self) {
        dialogs::forget(&self.tab_id);
        self.pending_evals.abort("the tab went away");
        // SAFETY: on the window's thread, with the page's own live objects.
        unsafe {
            let _ = self.controller.Close();
        }
        self.clip.destroy();
    }

    fn show(&self, visible: bool) {
        self.clip.show(visible);
        // SAFETY: on the window's thread, with the page's live controller.
        let _ = unsafe { self.controller.SetIsVisible(visible) };
    }

    /// Puts the clip window at `shown` and the page at `page`, both in the window's points.
    fn place(&self, shown: (f64, f64, f64, f64), page: (f64, f64, f64, f64)) {
        let scale = self.clip.scale();
        let px = |value: f64| (value * scale).round() as i32;
        self.clip
            .place(px(shown.0), px(shown.1), px(shown.2), px(shown.3));
        let bounds = RECT {
            left: px(page.0 - shown.0),
            top: px(page.1 - shown.1),
            right: px(page.0 - shown.0 + page.2),
            bottom: px(page.1 - shown.1 + page.3),
        };
        // SAFETY: on the window's thread, with the page's live controller.
        let _ = unsafe { self.controller.SetBounds(bounds) };
    }
}

/// A child window of the app's that a page is cut to.
#[derive(Clone, Copy)]
struct Clip {
    window: isize,
    parent: isize,
    beneath: bool,
}

impl Clip {
    fn new(parent: isize, beneath: bool) -> Result<Self, String> {
        // SAFETY: Win32 calls on the window's thread, under its own window `parent`; the class
        // lives as long as the program.
        let window = unsafe {
            let instance = GetModuleHandleW(None)
                .map(|module| HINSTANCE(module.0))
                .unwrap_or_default();
            let class = WNDCLASSW {
                lpfnWndProc: Some(clip_window),
                hInstance: instance,
                lpszClassName: w!("SikemuxPageClip"),
                ..Default::default()
            };
            RegisterClassW(&class);
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("SikemuxPageClip"),
                w!(""),
                WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                PARKED_ORIGIN as i32,
                PARKED_ORIGIN as i32,
                1,
                1,
                Some(HWND(parent as *mut _)),
                None,
                Some(instance),
                None,
            )
        }
        .map_err(|error| format!("the window cannot take a browser tab: {error}"))?;
        Ok(Self {
            window: window.0 as isize,
            parent,
            beneath,
        })
    }

    fn hwnd(&self) -> HWND {
        HWND(self.window as *mut _)
    }

    /// Physical pixels in one of the window's points.
    fn scale(&self) -> f64 {
        // SAFETY: asks about the app's live window.
        let dpi = unsafe { GetDpiForWindow(HWND(self.parent as *mut _)) };
        if dpi == 0 {
            1.0
        } else {
            f64::from(dpi) / 96.0
        }
    }

    fn place(&self, x: i32, y: i32, width: i32, height: i32) {
        let (after, order) = if self.beneath {
            (Some(HWND_BOTTOM), SWP_NOACTIVATE)
        } else {
            (None, SWP_NOACTIVATE | SWP_NOZORDER)
        };
        // SAFETY: moves our own child window.
        let _ =
            unsafe { SetWindowPos(self.hwnd(), after, x, y, width.max(0), height.max(0), order) };
    }

    fn show(&self, visible: bool) {
        // SAFETY: shows or hides our own child window.
        let _ = unsafe { ShowWindow(self.hwnd(), if visible { SW_SHOWNA } else { SW_HIDE }) };
    }

    fn destroy(&self) {
        // SAFETY: our own child window, destroyed on the thread that made it.
        let _ = unsafe { DestroyWindow(self.hwnd()) };
    }
}

unsafe extern "system" fn clip_window(
    window: HWND,
    message: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    // SAFETY: the system's default handling of a message to our own window.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

type Ready = Box<dyn FnOnce(Result<ICoreWebView2Environment, String>)>;

enum Environment {
    Starting(Vec<Ready>),
    Ready(ICoreWebView2Environment),
    Failed(String),
}

thread_local! {
    /// Every tab shares one browser and its signed-in profile.
    static ENVIRONMENT: RefCell<Option<Environment>> = const { RefCell::new(None) };
}

/// Hands `then` the browser, starting it on first use; it answers later, on this thread.
fn with_environment(then: impl FnOnce(Result<ICoreWebView2Environment, String>) + 'static) {
    let start = ENVIRONMENT.with_borrow_mut(|environment| match environment {
        Some(Environment::Ready(ready)) => {
            let ready = ready.clone();
            Some(Box::new(move || then(Ok(ready))) as Box<dyn FnOnce()>)
        }
        Some(Environment::Failed(why)) => {
            let why = why.clone();
            Some(Box::new(move || then(Err(why))) as Box<dyn FnOnce()>)
        }
        Some(Environment::Starting(waiting)) => {
            waiting.push(Box::new(then));
            None
        }
        None => {
            *environment = Some(Environment::Starting(vec![Box::new(then)]));
            Some(Box::new(start_environment) as Box<dyn FnOnce()>)
        }
    });
    if let Some(start) = start {
        start();
    }
}

fn start_environment() {
    let options = CoreWebView2EnvironmentOptions::default();
    let folder = profile_folder()
        .map(|folder| HSTRING::from(folder.as_os_str()))
        .unwrap_or_default();
    let handler =
        CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(|result, environment| {
            environment_ready(
                result
                    .map_err(|error| error.message())
                    .and_then(|()| environment.ok_or_else(|| "no browser came back".into())),
            );
            Ok(())
        }));
    // SAFETY: on the window's thread, which runs the messages the answer comes back through.
    let started = unsafe {
        options.set_additional_browser_arguments(BROWSER_ARGUMENTS.into());
        CreateCoreWebView2EnvironmentWithOptions(
            PCWSTR::null(),
            &folder,
            &ICoreWebView2EnvironmentOptions::from(options),
            &handler,
        )
    };
    if let Err(error) = started {
        environment_ready(Err(error.message()));
    }
}

fn environment_ready(result: Result<ICoreWebView2Environment, String>) {
    let waiting = ENVIRONMENT.with_borrow_mut(|environment| {
        let settled = match &result {
            Ok(ready) => Environment::Ready(ready.clone()),
            Err(why) => Environment::Failed(why.clone()),
        };
        match environment.replace(settled) {
            Some(Environment::Starting(waiting)) => waiting,
            _ => Vec::new(),
        }
    });
    for then in waiting {
        then(result.clone());
    }
}

/// Where the browser keeps its profile: cookies, storage and cache.
fn profile_folder() -> Option<PathBuf> {
    Some(sikemux_process::paths::data_dir()?.join("WebView2"))
}

struct Opening {
    held: Arc<Held>,
    parent: ParentView,
    agent_id: String,
    tab_id: String,
    url: String,
    user_agent: String,
    size: (f64, f64),
    private: bool,
    store: ProfileStore,
    events: Arc<dyn TabEvents>,
}

pub fn clear_data(profile: ProfileStore) {
    if let (ProfileStore::Named(uuid), Some(root)) = (profile, profile_folder()) {
        let dir = root.join("EBWebView").join(format!("p-{}", uuid.simple()));
        let _ = std::fs::remove_dir_all(dir);
    }
}

pub fn remove_data_store(profile: ProfileStore) {
    clear_data(profile);
}

impl Tab {
    /// Adds the tab's page to `parent`, hidden and parked off screen, and starts loading
    /// `spec.url`. The page is made in the background; the tab can be used at once.
    pub fn open(
        parent: ParentView,
        spec: TabSpec<'_>,
        events: Arc<dyn TabEvents>,
    ) -> Result<Tab, String> {
        if let Some(opener) = spec.opener {
            match opener {}
        }
        runtime_version()?;
        if !ui::started() {
            return Err(NO_THREAD.into());
        }
        let id: Arc<str> = spec.tab_id.into();
        let held = Arc::new(Held(Mutex::new(State::Opening(Vec::new()))));
        let opening = OnUi(Opening {
            held: held.clone(),
            parent,
            agent_id: spec.agent_id.to_owned(),
            tab_id: id.to_string(),
            url: spec.url.to_owned(),
            user_agent: spec.user_agent.to_owned(),
            size: spec.size,
            private: spec.private,
            store: spec.store,
            events,
        });
        on_ui(move || {
            let opening = opening.into_inner();
            let held = opening.held.clone();
            if let Err(why) = make(opening) {
                held.failed(why);
            }
        });
        Ok(Tab { id, held })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Runs `act` on the page on the window's thread, once the page is there.
    pub fn run(&self, act: impl FnOnce(&Page) + Send + 'static) {
        self.held.with(act);
    }

    /// Runs `act` on the page on the window's thread and hands back its answer.
    pub async fn ask<T: Send + 'static>(
        &self,
        act: impl FnOnce(&Page) -> T + Send + 'static,
    ) -> Result<T, String> {
        if let Some(why) = self.held.failure() {
            return Err(why);
        }
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.run(move |page| {
            let _ = sender.send(act(page));
        });
        receiver.await.map_err(|_| {
            self.held
                .failure()
                .unwrap_or_else(|| "the tab went away".into())
        })
    }

    pub fn show(&self) {
        self.held.with(|page| page.show(true));
    }

    pub fn hide(&self) {
        self.held.with(|page| page.show(false));
    }

    pub fn close(&self) {
        let held = self.held.clone();
        on_ui(move || held.close());
    }

    pub fn set_zoom(&self, zoom: f64) {
        self.held.with(move |page| {
            // SAFETY: on the window's thread, with the page's live controller.
            let _ = unsafe { page.controller.SetZoomFactor(zoom) };
        });
    }

    pub fn reload(&self) {
        self.held.with(|page| {
            // SAFETY: on the window's thread, with the page's live webview.
            let _ = unsafe { page.webview.Reload() };
        });
    }

    pub fn navigate(&self, url: &str) {
        let url = HSTRING::from(url);
        self.held.with(move |page| {
            // SAFETY: on the window's thread, with the page's live webview.
            let _ = unsafe { page.webview.Navigate(&url) };
        });
    }

    /// Puts the page at `frame` (x, y, width, height), top-down in the window, with `sides`
    /// (left, right) of it cut off.
    pub fn set_bounds(&self, frame: (f64, f64, f64, f64), sides: (f64, f64)) {
        let (x, y, width, height) = frame;
        let (left, right) = sides;
        self.held
            .with(move |page| page.place((x + left, y, width - left - right, height), frame));
    }

    /// Runs `script` in the page and hands back its value as JSON.
    pub async fn eval(&self, script: &str) -> Result<String, String> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let script = HSTRING::from(script);
        self.held.with(move |page| {
            let handler = ExecuteScriptCompletedHandler::create(Box::new(move |result, json| {
                let _ = sender.send(result.map(|()| json).map_err(|error| error.message()));
                Ok(())
            }));
            // SAFETY: on the window's thread, with the page's live webview.
            if let Err(error) = unsafe { page.webview.ExecuteScript(&script, &handler) } {
                eprintln!("sikemux: a script could not start: {error}");
            }
        });
        if let Some(why) = self.held.failure() {
            return Err(why);
        }
        receiver
            .await
            .map_err(|_| "the tab went away".to_string())?
    }
}

/// Makes the page's clip window, then its WebView2 once the browser is ready.
fn make(opening: Opening) -> Result<(), String> {
    let clip = Clip::new(opening.parent.view as isize, opening.parent.beneath)?;
    let held = opening.held.clone();
    with_environment(move |environment| {
        let made = environment.and_then(|environment| make_controller(&environment, clip, opening));
        if let Err(why) = made {
            clip.destroy();
            held.failed(why);
        }
    });
    Ok(())
}

fn make_controller(
    environment: &ICoreWebView2Environment,
    clip: Clip,
    opening: Opening,
) -> Result<(), String> {
    let environment = environment
        .cast::<ICoreWebView2Environment10>()
        .map_err(|_| format!("{NO_RUNTIME} in version 101 or later"))?;
    let held = opening.held.clone();
    // SAFETY: on the window's thread; the controller comes back through its messages.
    unsafe {
        let options = environment
            .CreateCoreWebView2ControllerOptions()
            .map_err(|error| error.message())?;
        let _ = options.SetIsInPrivateModeEnabled(opening.private);
        if let ProfileStore::Named(uuid) = opening.store {
            let name = HSTRING::from(format!("p-{}", uuid.simple()));
            let _ = options.SetProfileName(&name);
        }
        if let (Some([r, g, b]), Ok(colored)) = (
            opening.parent.ground,
            options.cast::<ICoreWebView2ControllerOptions3>(),
        ) {
            let _ = colored.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR {
                A: 255,
                R: r,
                G: g,
                B: b,
            });
        }
        let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
            move |result, controller| {
                let made = result
                    .map_err(|error| error.message())
                    .and_then(|()| controller.ok_or_else(|| "no page came back".into()))
                    .and_then(|controller| set_up(controller, clip, &opening));
                match made {
                    Ok(page) => opening.held.opened(page),
                    Err(why) => {
                        clip.destroy();
                        opening.held.failed(why);
                    }
                }
                Ok(())
            },
        ));
        environment
            .CreateCoreWebView2ControllerWithOptions(clip.hwnd(), &options, &handler)
            .map_err(|error| error.message())?;
    }
    let _ = held;
    Ok(())
}

/// Readies a new page: its settings, what it reports to the tab, and its first load.
fn set_up(
    controller: ICoreWebView2Controller,
    clip: Clip,
    opening: &Opening,
) -> Result<Page, String> {
    // SAFETY: on the window's thread, with the controller just made.
    let webview = unsafe { controller.CoreWebView2() }.map_err(|error| error.message())?;
    let page = Page {
        tab_id: opening.tab_id.clone(),
        controller,
        webview,
        clip,
        helper: Arc::default(),
        pending_evals: Arc::default(),
    };
    // SAFETY: on the window's thread, with the page's live objects.
    unsafe {
        let settings = page.webview.Settings().map_err(|error| error.message())?;
        let _ = settings.SetIsStatusBarEnabled(false);
        let _ = settings.SetAreDefaultScriptDialogsEnabled(false);
        page::introduce_as(&page, &opening.user_agent);
        let _ = page.webview.AddScriptToExecuteOnDocumentCreated(
            &HSTRING::from(crate::recorder_script()),
            &AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(|_, _| Ok(()))),
        );
    }
    listen(
        &page,
        &opening.agent_id,
        &opening.tab_id,
        opening.events.clone(),
    )?;
    page.show(false);
    page.place(
        (PARKED_ORIGIN, PARKED_ORIGIN, opening.size.0, opening.size.1),
        (PARKED_ORIGIN, PARKED_ORIGIN, opening.size.0, opening.size.1),
    );
    // SAFETY: on the window's thread, with the page's live webview.
    unsafe { page.webview.Navigate(&HSTRING::from(opening.url.as_str())) }
        .map_err(|error| error.message())?;
    Ok(page)
}

/// Hands what the page does on to the tab's events.
fn listen(
    page: &Page,
    agent_id: &str,
    tab_id: &str,
    events: Arc<dyn TabEvents>,
) -> Result<(), String> {
    let webview = &page.webview;
    let mut token = 0;
    let history = |webview: &ICoreWebView2| {
        let (mut back, mut forward) = (Default::default(), Default::default());
        // SAFETY: reads the live webview's history on the window's thread.
        unsafe {
            let _ = webview.CanGoBack(&mut back);
            let _ = webview.CanGoForward(&mut forward);
        }
        (back.as_bool(), forward.as_bool())
    };
    let source = |webview: &ICoreWebView2| {
        let mut url = PWSTR::null();
        // SAFETY: the webview fills `url` with a string `take_pwstr` frees.
        let _ = unsafe { webview.Source(&mut url) };
        take_pwstr(url)
    };
    // SAFETY: registers handlers on the live webview, on the window's thread; each runs there.
    unsafe {
        let starting = events.clone();
        let nav_evals = page.pending_evals.clone();
        let nav_helper = page.helper.clone();
        webview
            .add_NavigationStarting(
                &NavigationStartingEventHandler::create(Box::new(move |webview, args| {
                    let (Some(webview), Some(args)) = (webview, args) else {
                        return Ok(());
                    };
                    let mut url = PWSTR::null();
                    args.Uri(&mut url)?;
                    let url = take_pwstr(url);
                    if !starting.may_load(&url) {
                        return args.SetCancel(true);
                    }
                    *nav_helper.locked() = None;
                    nav_evals.abort(crate::native::EVAL_NAVIGATED);
                    let mut navigation = 0;
                    args.NavigationId(&mut navigation)?;
                    starting.document(DocumentEvent::Started {
                        navigation: navigation as usize,
                        url: url.clone(),
                    });
                    let (can_go_back, can_go_forward) = history(&webview);
                    starting.loaded(PageLoad {
                        started: true,
                        url,
                        can_go_back,
                        can_go_forward,
                    });
                    Ok(())
                })),
                &mut token,
            )
            .map_err(|error| error.message())?;
        let completed = events.clone();
        webview
            .add_NavigationCompleted(
                &NavigationCompletedEventHandler::create(Box::new(move |webview, args| {
                    let (Some(webview), Some(args)) = (webview, args) else {
                        return Ok(());
                    };
                    let (mut navigation, mut success) = (0, Default::default());
                    args.NavigationId(&mut navigation)?;
                    args.IsSuccess(&mut success)?;
                    let navigation = navigation as usize;
                    let url = source(&webview);
                    if let Ok(status) = args.cast::<ICoreWebView2NavigationCompletedEventArgs2>() {
                        let mut code = 0;
                        if status.HttpStatusCode(&mut code).is_ok() && code > 0 {
                            completed.document(DocumentEvent::Responded {
                                url: url.clone(),
                                status: u16::try_from(code).ok(),
                                mime_type: String::new(),
                            });
                        }
                    }
                    if success.as_bool() {
                        completed.document(DocumentEvent::Finished { navigation });
                    } else {
                        let mut error = COREWEBVIEW2_WEB_ERROR_STATUS::default();
                        args.WebErrorStatus(&mut error)?;
                        completed.document(DocumentEvent::Failed {
                            navigation,
                            url: (!url.is_empty()).then_some(url.clone()),
                            error: format!("the page did not load (WebView2 error {})", error.0),
                            provisional: true,
                        });
                    }
                    let (can_go_back, can_go_forward) = history(&webview);
                    completed.loaded(PageLoad {
                        started: false,
                        url,
                        can_go_back,
                        can_go_forward,
                    });
                    Ok(())
                })),
                &mut token,
            )
            .map_err(|error| error.message())?;
        let moved = events.clone();
        webview
            .add_SourceChanged(
                &SourceChangedEventHandler::create(Box::new(move |webview, args| {
                    let (Some(webview), Some(args)) = (webview, args) else {
                        return Ok(());
                    };
                    let mut new_document = Default::default();
                    args.IsNewDocument(&mut new_document)?;
                    if !new_document.as_bool() {
                        let (back, forward) = history(&webview);
                        moved.moved(source(&webview), back, forward);
                    }
                    Ok(())
                })),
                &mut token,
            )
            .map_err(|error| error.message())?;
        let titled = events.clone();
        webview
            .add_DocumentTitleChanged(
                &DocumentTitleChangedEventHandler::create(Box::new(move |webview, _| {
                    if let Some(webview) = webview {
                        let mut title = PWSTR::null();
                        webview.DocumentTitle(&mut title)?;
                        titled.titled(take_pwstr(title));
                    }
                    Ok(())
                })),
                &mut token,
            )
            .map_err(|error| error.message())?;
        let popup = events.clone();
        webview
            .add_NewWindowRequested(
                &NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
                    if let Some(args) = args {
                        let mut url = PWSTR::null();
                        args.Uri(&mut url)?;
                        args.SetHandled(true)?;
                        popup.new_window(take_pwstr(url), None);
                    }
                    Ok(())
                })),
                &mut token,
            )
            .map_err(|error| error.message())?;
        let closing = events.clone();
        webview
            .add_WindowCloseRequested(
                &WindowCloseRequestedEventHandler::create(Box::new(move |_, _| {
                    closing.closed();
                    Ok(())
                })),
                &mut token,
            )
            .map_err(|error| error.message())?;
        let failing = events.clone();
        let crash_evals = page.pending_evals.clone();
        webview
            .add_ProcessFailed(
                &ProcessFailedEventHandler::create(Box::new(move |_, args| {
                    if let Some(args) = args {
                        let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
                        args.ProcessFailedKind(&mut kind)?;
                        if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE {
                            failing.health(Some(TabStall::Unresponsive));
                        } else if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED
                            || kind == COREWEBVIEW2_PROCESS_FAILED_KIND_FRAME_RENDER_PROCESS_EXITED
                        {
                            crash_evals.abort("the tab's page crashed while the script ran");
                            failing.health(Some(TabStall::Crashed));
                        }
                    }
                    Ok(())
                })),
                &mut token,
            )
            .map_err(|error| error.message())?;
        if let Ok(downloads) = webview.cast::<ICoreWebView2_4>() {
            let saving = events.clone();
            downloads
                .add_DownloadStarting(
                    &DownloadStartingEventHandler::create(Box::new(move |_, args| {
                        let Some(args) = args else {
                            return Ok(());
                        };
                        download_starting(&args, saving.clone())
                    })),
                    &mut token,
                )
                .map_err(|error| error.message())?;
        }
        page::listen_for_shortcuts(page, agent_id, tab_id)?;
        dialogs::listen(page, tab_id, events)?;
    }
    Ok(())
}

/// Saves where the tab's events say, or refuses, and reports how it ended.
fn download_starting(
    args: &ICoreWebView2DownloadStartingEventArgs,
    events: Arc<dyn TabEvents>,
) -> windows::core::Result<()> {
    // SAFETY: reads and answers the live event on the window's thread.
    unsafe {
        let operation = args.DownloadOperation()?;
        let mut url = PWSTR::null();
        operation.Uri(&mut url)?;
        let url = take_pwstr(url);
        let mut suggested = PWSTR::null();
        args.ResultFilePath(&mut suggested)?;
        let suggested = PathBuf::from(take_pwstr(suggested));
        args.SetHandled(true)?;
        let Some(chosen) = events.download_started(url.clone(), &suggested) else {
            return args.SetCancel(true);
        };
        args.SetResultFilePath(&HSTRING::from(chosen.as_os_str()))?;
        let mut token = 0;
        operation.add_StateChanged(
            &StateChangedEventHandler::create(Box::new(move |operation, _| {
                let Some(operation) = operation else {
                    return Ok(());
                };
                let mut state = COREWEBVIEW2_DOWNLOAD_STATE::default();
                operation.State(&mut state)?;
                if state != COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS {
                    events.download_finished(
                        url.clone(),
                        state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED,
                    );
                }
                Ok(())
            })),
            &mut token,
        )
    }
}

/// Runs a DevTools method on the page and hands back its answer.
pub(crate) fn devtools(
    page: &Page,
    method: &str,
    params: &Value,
    done: impl FnOnce(Result<Value, String>) + 'static,
) {
    let done = RefCell::new(Some(done));
    let handler =
        CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, json| {
            if let Some(done) = done.take() {
                done(
                    result.map_err(|error| error.message()).and_then(|()| {
                        serde_json::from_str(&json).map_err(|error| error.to_string())
                    }),
                );
            }
            Ok(())
        }));
    // SAFETY: on the window's thread, with the page's live webview.
    let called = unsafe {
        page.webview.CallDevToolsProtocolMethod(
            &HSTRING::from(method),
            &HSTRING::from(params.to_string()),
            &handler,
        )
    };
    if let Err(error) = called {
        eprintln!("sikemux: the page did not take {method}: {error}");
    }
}

pub enum PageView {}

impl PageView {
    pub fn show(&self, _html: &str) {
        match *self {}
    }

    pub fn run_script(&self, _script: &str) {
        match *self {}
    }
}
