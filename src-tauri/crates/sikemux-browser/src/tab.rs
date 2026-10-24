//! A tab's page. The engine makes its WKWebView and keeps it on the main
//! thread; the handle can be held and cloned on any thread, and everything it
//! does to the page runs on the main one.

#[cfg(not(target_os = "linux"))]
use std::sync::Arc;

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2::MainThreadMarker;
#[cfg(target_os = "macos")]
use objc2_app_kit::NSView;
#[cfg(target_os = "macos")]
use objc2_web_kit::WKWebView;

#[cfg(not(target_os = "linux"))]
use crate::TabEvents;

/// The window view tabs are added to: an `NSView` on macOS, the window's
/// `GtkBox` on Linux.
#[derive(Clone, Copy)]
pub struct ParentView {
    pub view: *mut std::ffi::c_void,
    /// Tabs go under the view's other children, such as a plane drawing popovers
    /// over them, instead of on top.
    pub beneath: bool,
}

impl ParentView {
    pub fn on_top(view: *mut std::ffi::c_void) -> Self {
        Self {
            view,
            beneath: false,
        }
    }

    pub fn beneath(view: *mut std::ffi::c_void) -> Self {
        Self {
            view,
            beneath: true,
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
}

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
        let (agent_id, url, user_agent, size) = (
            spec.agent_id.to_owned(),
            spec.url.to_owned(),
            spec.user_agent.to_owned(),
            spec.size,
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
                &url,
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
        MainThreadMarker::new()?;
        self.view.0.clone().map(Retained::into_super)
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
            crate::macos::forget(&id);
            webview.removeFromSuperview();
        });
    }

    pub fn set_zoom(&self, zoom: f64) {
        // SAFETY: `run` calls this on the main thread with the live webview.
        self.run(move |webview| unsafe { webview.setPageZoom(zoom) });
    }

    pub fn reload(&self) {
        // SAFETY: as above.
        self.run(|webview| {
            let _ = unsafe { webview.reload() };
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
pub use linux::Tab;

/// On Linux a tab is WebKitGTK through wry, placed in the window's GTK box as
/// Tauri placed its child webviews, with the GTK main context doing the work.
#[cfg(target_os = "linux")]
mod linux {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use gtk::glib;
    use sikemux_process::sync::Locked;

    use super::{ParentView, TabSpec};
    use crate::{PageLoad, TabEvents, PARKED_ORIGIN};

    #[derive(Clone)]
    pub struct Tab {
        id: Arc<str>,
        view: Arc<Held>,
    }

    /// The page's view, dropped on the main thread whichever thread lets go last.
    struct Held(Mutex<Option<wry::WebView>>);

    // SAFETY: the view inside is only touched on the main thread: through `on_main`,
    // and in `Drop`, which sends it there.
    unsafe impl Send for Held {}
    unsafe impl Sync for Held {}

    impl Drop for Held {
        fn drop(&mut self) {
            let Some(view) = self.0.locked().take() else {
                return;
            };
            if glib::MainContext::default().is_owner() {
                drop(view);
            } else {
                let held = Held(Mutex::new(Some(view)));
                glib::MainContext::default().invoke(move || drop(held));
            }
        }
    }

    fn on_main(work: impl FnOnce() + Send + 'static) {
        let context = glib::MainContext::default();
        if context.is_owner() {
            work();
        } else {
            context.invoke(work);
        }
    }

    impl Tab {
        pub fn open(
            parent: ParentView,
            spec: TabSpec<'_>,
            events: Arc<dyn TabEvents>,
        ) -> Result<Tab, String> {
            let id: Arc<str> = spec.tab_id.into();
            let (url, size) = (spec.url.to_owned(), spec.size);
            let make = move || -> Result<Held, String> {
                let parent = parent;
                // SAFETY: the caller hands over the live `GtkBox` of an open window, and
                // this runs on the main thread.
                let container: gtk::Box = unsafe {
                    glib::translate::from_glib_none(parent.view.cast::<gtk::ffi::GtkBox>())
                };
                let (load, title, popup, started, finished) = (
                    events.clone(),
                    events.clone(),
                    events.clone(),
                    events.clone(),
                    events.clone(),
                );
                let view = wry::WebViewBuilder::new()
                    .with_url(&url)
                    .with_initialization_script(crate::RECORDER_SCRIPT)
                    .with_focused(false)
                    .with_visible(false)
                    .with_hotkeys_zoom(true)
                    .with_bounds(wry::Rect {
                        position: wry::dpi::LogicalPosition::new(PARKED_ORIGIN, PARKED_ORIGIN)
                            .into(),
                        size: wry::dpi::LogicalSize::new(size.0, size.1).into(),
                    })
                    .with_navigation_handler(move |url| events.may_load(&url))
                    .with_on_page_load_handler(move |event, url| {
                        load.loaded(PageLoad {
                            started: matches!(event, wry::PageLoadEvent::Started),
                            url,
                            can_go_back: false,
                            can_go_forward: false,
                        })
                    })
                    .with_document_title_changed_handler(move |name| title.titled(name))
                    .with_new_window_req_handler(move |url, _| {
                        popup.new_window(url);
                        wry::NewWindowResponse::Deny
                    })
                    .with_download_started_handler(move |url, path: &mut PathBuf| {
                        match started.download_started(url, path) {
                            Some(chosen) => {
                                *path = chosen;
                                true
                            }
                            None => false,
                        }
                    })
                    .with_download_completed_handler(move |url, _, saved| {
                        finished.download_finished(url, saved)
                    });
                let view = wry::WebViewBuilderExtUnix::build_gtk(view, &container)
                    .map_err(|error| error.to_string())?;
                Ok(Held(Mutex::new(Some(view))))
            };
            let context = glib::MainContext::default();
            let held = if context.is_owner() {
                make()?
            } else {
                let (sender, receiver) = std::sync::mpsc::channel();
                context.invoke(move || {
                    let _ = sender.send(make());
                });
                receiver
                    .recv()
                    .map_err(|_| "the window cannot take a browser tab".to_string())??
            };
            Ok(Tab {
                id,
                view: Arc::new(held),
            })
        }

        pub fn id(&self) -> &str {
            &self.id
        }

        fn with(&self, act: impl FnOnce(&wry::WebView) + Send + 'static) {
            let view = self.view.clone();
            on_main(move || {
                if let Some(webview) = view.0.locked().as_ref() {
                    act(webview);
                }
            });
        }

        pub fn show(&self) {
            self.with(|webview| {
                let _ = webview.set_visible(true);
            });
        }

        pub fn hide(&self) {
            self.with(|webview| {
                let _ = webview.set_visible(false);
            });
        }

        /// Takes the page out of its window.
        pub fn close(&self) {
            let view = self.view.clone();
            on_main(move || drop(view.0.locked().take()));
        }

        pub fn set_zoom(&self, zoom: f64) {
            self.with(move |webview| {
                let _ = webview.zoom(zoom);
            });
        }

        pub fn reload(&self) {
            self.with(|webview| {
                let _ = webview.reload();
            });
        }

        pub fn navigate(&self, url: &str) {
            let url = url.to_owned();
            self.with(move |webview| {
                let _ = webview.load_url(&url);
            });
        }

        /// Puts the page at `frame` (x, y, width, height), top-down in the window.
        pub fn set_bounds(&self, frame: (f64, f64, f64, f64)) {
            let (x, y, width, height) = frame;
            self.with(move |webview| {
                let _ = webview.set_bounds(wry::Rect {
                    position: wry::dpi::LogicalPosition::new(x, y).into(),
                    size: wry::dpi::LogicalSize::new(width, height).into(),
                });
            });
        }

        /// Runs `script` in the page and hands back its value as JSON, or an empty
        /// string when it has none.
        pub async fn eval(&self, script: &str) -> Result<String, String> {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            let script = script.to_owned();
            self.with(move |webview| {
                let sender = Mutex::new(Some(sender));
                let _ = webview.evaluate_script_with_callback(&script, move |json| {
                    if let Some(sender) = sender.locked().take() {
                        let _ = sender.send(json);
                    }
                });
            });
            receiver.await.map_err(|_| "the tab went away".to_string())
        }
    }
}

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

    pub fn set_bounds(&self, _frame: (f64, f64, f64, f64)) {
        match self.0 {}
    }

    pub async fn eval(&self, _script: &str) -> Result<String, String> {
        match self.0 {}
    }
}
