//! A tab's page. The engine makes its webview and keeps it on the thread its toolkit runs
//! on; the handle can be held and cloned on any thread, and everything it does to the page
//! runs on that one.

#[cfg(not(target_os = "linux"))]
use std::sync::Arc;

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2::MainThreadMarker;
#[cfg(target_os = "macos")]
use objc2_app_kit::NSView;
#[cfg(target_os = "macos")]
use objc2_web_kit::{WKWebView, WKWebViewConfiguration};

#[cfg(not(target_os = "linux"))]
use crate::TabEvents;

/// The window view tabs are added to: an `NSView` on macOS, the window's X11 id on Linux.
#[derive(Clone, Copy)]
pub struct ParentView {
    pub view: *mut std::ffi::c_void,
    /// Tabs go under the view's other children, such as a plane drawing popovers
    /// over them, instead of on top.
    pub beneath: bool,
    /// The colour the window shows where a page goes, which a page on Linux shows
    /// until it draws its first frame.
    pub ground: Option<[u8; 3]>,
}

impl ParentView {
    pub fn beneath(view: *mut std::ffi::c_void) -> Self {
        Self {
            view,
            beneath: true,
            ground: None,
        }
    }

    pub fn with_ground(self, ground: [u8; 3]) -> Self {
        Self {
            ground: Some(ground),
            ..self
        }
    }
}

// SAFETY: only the pointer value crosses threads; it is used on the main thread alone.
unsafe impl Send for ParentView {}

pub struct TabSpec<'a> {
    pub agent_id: &'a str,
    pub tab_id: &'a str,
    pub url: &'a str,
    pub user_agent: &'a str,
    /// The size the page lays out at while it waits off screen to be shown.
    pub size: (f64, f64),
    /// The page that opened this tab as its window. The engine then loads `url` itself.
    pub opener: Option<Opener>,
}

/// What a page that opens a window hands over for it. A tab made from it stays linked
/// to that page, so a sign-in popup can post its answer back to the page that opened it.
#[cfg(target_os = "macos")]
pub struct Opener(pub(crate) Retained<WKWebViewConfiguration>);

// SAFETY: WebKit hands an opener over on the main thread, and the tab is made from it
// there before the call returns, so it never reaches another thread.
#[cfg(target_os = "macos")]
unsafe impl Send for Opener {}

#[cfg(not(target_os = "macos"))]
pub enum Opener {}

#[cfg(target_os = "macos")]
#[derive(Clone)]
pub struct Tab {
    id: Arc<str>,
    view: Arc<Held>,
}

/// The page's view, released on the main thread whichever thread lets go last.
#[cfg(target_os = "macos")]
struct Held(Option<Retained<WKWebView>>);

// SAFETY: the view inside is only touched on the main thread: through `on_main`, and
// in `Drop`, which sends it there.
#[cfg(target_os = "macos")]
unsafe impl Send for Held {}
#[cfg(target_os = "macos")]
unsafe impl Sync for Held {}

#[cfg(target_os = "macos")]
impl Drop for Held {
    fn drop(&mut self) {
        let Some(view) = self.0.take() else {
            return;
        };
        if MainThreadMarker::new().is_some() {
            drop(view);
        } else {
            let held = Held(Some(view));
            dispatch2::DispatchQueue::main().exec_async(move || drop(held));
        }
    }
}

#[cfg(target_os = "macos")]
fn on_main(work: impl FnOnce() + Send + 'static) {
    if MainThreadMarker::new().is_some() {
        work();
    } else {
        dispatch2::DispatchQueue::main().exec_async(work);
    }
}

#[cfg(target_os = "macos")]
impl Tab {
    /// Adds the tab's page to `parent`, hidden and parked off screen, and
    /// starts loading `spec.url`. Waits for the main thread when called off it.
    pub fn open(
        parent: ParentView,
        spec: TabSpec<'_>,
        events: Arc<dyn TabEvents>,
    ) -> Result<Tab, String> {
        let id: Arc<str> = spec.tab_id.into();
        let (agent_id, url, user_agent, size, opener) = (
            spec.agent_id.to_owned(),
            spec.url.to_owned(),
            spec.user_agent.to_owned(),
            spec.size,
            spec.opener,
        );
        let tab_id = id.to_string();
        let make = move || {
            let parent = parent;
            MainThreadMarker::new()?;
            // SAFETY: the caller hands over a live NSView of an open window, and this
            // runs on the main thread.
            let view = unsafe { parent.view.cast::<NSView>().as_ref() }?;
            Some(Held(Some(crate::macos::open(
                crate::macos::Parent {
                    view,
                    beneath: parent.beneath,
                },
                agent_id,
                tab_id,
                match opener {
                    Some(opener) => crate::macos::Start::Opened(opener),
                    None => crate::macos::Start::Load(&url),
                },
                &user_agent,
                size,
                events,
            ))))
        };
        let held = if MainThreadMarker::new().is_some() {
            make()
        } else {
            let mut made = None;
            dispatch2::DispatchQueue::main().exec_sync(|| made = make());
            made
        };
        let view = held.ok_or("the window cannot take a browser tab")?;
        Ok(Tab {
            id,
            view: Arc::new(view),
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// The page's view, to code already on the main thread.
    pub fn view_here(&self) -> Option<Retained<NSView>> {
        self.webview_here().map(Retained::into_super)
    }

    pub(crate) fn webview_here(&self) -> Option<Retained<WKWebView>> {
        MainThreadMarker::new()?;
        self.view.0.clone()
    }

    /// Runs `act` on the page on the main thread, at once when called there.
    pub fn run(&self, act: impl FnOnce(&WKWebView) + Send + 'static) {
        let view = self.view.clone();
        on_main(move || {
            if let Some(webview) = view.0.as_deref() {
                act(webview);
            }
        });
    }

    /// Runs `act` on the page on the main thread and hands back its answer.
    pub async fn ask<T: Send + 'static>(
        &self,
        act: impl FnOnce(&WKWebView) -> T + Send + 'static,
    ) -> Result<T, String> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.run(move |webview| {
            let _ = sender.send(act(webview));
        });
        receiver.await.map_err(|_| "the tab went away".to_string())
    }

    pub fn show(&self) {
        self.run(|webview| webview.setHidden(false));
    }

    pub fn hide(&self) {
        self.run(|webview| webview.setHidden(true));
    }

    /// Takes the page out of its window and lets go of what watched it.
    pub fn close(&self) {
        let id = self.id.clone();
        self.run(move |webview| {
            crate::macos::out_of_fullscreen(webview, move |webview| {
                crate::macos::forget(&id);
                webview.removeFromSuperview();
            })
        });
    }

    pub fn set_zoom(&self, zoom: f64) {
        // SAFETY: `run` calls this on the main thread with the live webview.
        self.run(move |webview| unsafe { webview.setPageZoom(zoom) });
    }

    pub fn reload(&self) {
        self.run(|webview| {
            // SAFETY: as above.
            crate::macos::out_of_fullscreen(webview, |webview| unsafe {
                let _ = webview.reload();
            })
        });
    }

    pub fn navigate(&self, url: &str) {
        let url = url.to_owned();
        self.run(move |webview| crate::macos::load(webview, &url));
    }

    /// Puts the page at `frame` (x, y, width, height), top-down in the window.
    pub fn set_bounds(&self, frame: (f64, f64, f64, f64)) {
        use objc2_foundation::{NSPoint, NSRect, NSSize};
        let (x, y, width, height) = frame;
        self.run(move |webview| {
            crate::macos::place(
                webview,
                NSRect::new(NSPoint::new(x, y), NSSize::new(width, height)),
                None,
            )
        });
    }

    /// Runs `script` in the page and hands back its value as JSON, or an empty
    /// string when it has none.
    pub async fn eval(&self, script: &str) -> Result<String, String> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let script = script.to_owned();
        self.run(move |webview| {
            crate::macos::evaluate(
                webview,
                &script,
                Box::new(move |json| {
                    let _ = sender.send(json);
                }),
            )
        });
        receiver.await.map_err(|_| "the tab went away".to_string())
    }
}

#[cfg(target_os = "linux")]
pub use crate::linux::Tab;

/// Elsewhere there is no browser engine yet, so no tab can be made.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
#[derive(Clone)]
pub struct Tab(Never);

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
#[derive(Clone)]
enum Never {}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
impl Tab {
    pub fn open(
        _parent: ParentView,
        _spec: TabSpec<'_>,
        _events: Arc<dyn TabEvents>,
    ) -> Result<Tab, String> {
        Err("browser tabs need macOS or Linux".into())
    }

    pub fn id(&self) -> &str {
        match self.0 {}
    }

    pub fn show(&self) {
        match self.0 {}
    }

    pub fn hide(&self) {
        match self.0 {}
    }

    pub fn close(&self) {
        match self.0 {}
    }

    pub fn set_zoom(&self, _zoom: f64) {
        match self.0 {}
    }

    pub fn reload(&self) {
        match self.0 {}
    }

    pub fn navigate(&self, _url: &str) {
        match self.0 {}
    }

    pub fn set_bounds(&self, _frame: (f64, f64, f64, f64), _sides: (f64, f64)) {
        match self.0 {}
    }

    pub async fn eval(&self, _script: &str) -> Result<String, String> {
        match self.0 {}
    }
}
