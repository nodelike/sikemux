//! A tab's page on Linux: WebKitGTK through wry, in an X11 window of its own inside the app's
//! window. GTK runs on a thread of its own, which every call into a page goes to.
//!
//! Each page sits in a clip window of ours, as wide as the part of the page that shows, so a
//! page reaching past the stage's edge is cut there. The app's own drawing cuts the clip
//! window where its menus show over the page.

pub(crate) mod dialogs;
pub(crate) mod input;
pub(crate) mod page;

use std::num::NonZeroU32;
use std::os::raw::c_ulong;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use gtk::glib;
use gtk::prelude::{Cast, WidgetExt};
use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, WindowHandle, XlibWindowHandle,
};
use sikemux_process::sync::Locked;
use webkit2gtk::WebView;
use wry::WebViewExtUnix;
use x11::xlib::{self, Display};

use crate::tab::{ParentView, TabSpec};
use crate::{PageLoad, TabEvents};

/// X11 places windows with 16-bit coordinates, so a page parked any further out comes back
/// into sight.
const PARKED: f64 = -30_000.0;

#[derive(Clone)]
pub struct Tab {
    id: Arc<str>,
    view: Arc<Held>,
}

/// The page's view and its clip window, dropped on the GTK thread whichever thread lets go
/// last.
pub(crate) struct Held(Mutex<Option<Page>>);

impl Held {
    pub(crate) fn new(page: Page) -> Self {
        Self(Mutex::new(Some(page)))
    }

    /// Runs `act` on the page on the GTK thread, unless it has been closed.
    pub(crate) fn with(self: &Arc<Self>, act: impl FnOnce(&Page) + Send + 'static) {
        let held = self.clone();
        on_gtk(move || {
            if let Some(page) = held.0.locked().as_ref() {
                act(page);
            }
        });
    }
}

/// The page goes before the window it sits in.
pub(crate) struct Page {
    pub(crate) view: wry::WebView,
    pub(crate) clip: Clip,
}

impl Page {
    /// WebKit can take seconds to draw a page's first frame on a cold or slow machine, and
    /// until then X shows the page's window in its own colour, black. It shows `ground` instead.
    pub(crate) fn new(view: wry::WebView, clip: Clip, ground: Option<[u8; 3]>) -> Self {
        let window = view
            .webview()
            .toplevel()
            .and_then(|toplevel| toplevel.window())
            .and_then(|window| window.downcast::<gdkx11::X11Window>().ok());
        if let (Some(ground), Some(window)) = (ground, window) {
            paint(clip.display, window.xid(), ground);
        }
        Self { view, clip }
    }

    /// Shows the part of the page that falls in `shown` (x, y, width, height), the page itself
    /// being at `page`, both top-down in the app's window.
    pub(crate) fn place(&self, shown: (f64, f64, f64, f64), page: (f64, f64, f64, f64)) {
        let (x, y, width, height) = shown;
        let (x, y) = (x.max(PARKED), y.max(PARKED));
        let (left, top) = (page.0.max(PARKED), page.1.max(PARKED));
        self.clip.place(x, y, width, height);
        let _ = self.view.set_bounds(wry::Rect {
            position: wry::dpi::LogicalPosition::new(left - x, top - y).into(),
            size: wry::dpi::LogicalSize::new(page.2, page.3).into(),
        });
    }

    pub(crate) fn show(&self, shown: bool) {
        if shown {
            self.clip.show(true);
            let _ = self.view.set_visible(true);
        } else {
            let _ = self.view.set_visible(false);
            self.clip.show(false);
        }
    }
}

/// A window of ours between the app's window and the page's.
pub(crate) struct Clip {
    display: *mut Display,
    pub(crate) window: c_ulong,
}

impl Clip {
    /// Adds a clip window to `parent`, out of sight. It draws nothing of its own.
    pub(crate) fn new(parent: c_ulong, ground: Option<[u8; 3]>) -> Result<Self, String> {
        let display = crate::linux::page::xdisplay().ok_or("GTK has no X11 display")?;
        // SAFETY: GTK's live X11 display on the GTK thread, and `parent` is the app's live window.
        let window = unsafe {
            let window = xlib::XCreateSimpleWindow(
                display,
                parent,
                PARKED as i32,
                PARKED as i32,
                1,
                1,
                0,
                0,
                0,
            );
            xlib::XSetWindowBackgroundPixmap(display, window, 0);
            xlib::XFlush(display);
            window
        };
        if let Some(ground) = ground {
            paint(display, window, ground);
        }
        Ok(Self { display, window })
    }

    fn place(&self, x: f64, y: f64, width: f64, height: f64) {
        // SAFETY: our live window on GTK's display, on the GTK thread.
        unsafe {
            xlib::XMoveResizeWindow(
                self.display,
                self.window,
                x as i32,
                y as i32,
                width.max(1.) as u32,
                height.max(1.) as u32,
            );
            xlib::XFlush(self.display);
        }
    }

    fn show(&self, shown: bool) {
        // SAFETY: as above.
        unsafe {
            if shown {
                xlib::XMapWindow(self.display, self.window);
            } else {
                xlib::XUnmapWindow(self.display, self.window);
            }
            xlib::XFlush(self.display);
        }
    }
}

/// Gives `window` the background `ground`, as its depth holds a colour.
fn paint(display: *mut Display, window: c_ulong, ground: [u8; 3]) {
    let [red, green, blue] = ground.map(c_ulong::from);
    let colour = red << 16 | green << 8 | blue;
    // SAFETY: a live window on GTK's display, on the GTK thread; X fills the attributes.
    unsafe {
        let mut attributes: xlib::XWindowAttributes = std::mem::zeroed();
        xlib::XGetWindowAttributes(display, window, &mut attributes);
        let opaque = if attributes.depth == 32 {
            0xff << 24
        } else {
            0
        };
        xlib::XSetWindowBackground(display, window, opaque | colour);
        xlib::XClearWindow(display, window);
        xlib::XFlush(display);
    }
}

impl Drop for Clip {
    fn drop(&mut self) {
        // SAFETY: as above; nothing uses the window after this.
        unsafe {
            xlib::XDestroyWindow(self.display, self.window);
            xlib::XFlush(self.display);
        }
    }
}

// SAFETY: the view inside is only touched on the GTK thread: through `on_gtk`, and in
// `Drop`, which sends it there.
unsafe impl Send for Held {}
unsafe impl Sync for Held {}

impl Drop for Held {
    fn drop(&mut self) {
        if let Some(page) = self.0.locked().take() {
            let held = Held::new(page);
            on_gtk(move || drop(held));
        }
    }
}

/// WebKitGTK draws pages into DMA-BUF buffers when it finds a render node, and those only show
/// where the X server has DRI3. Without it, as on virtual machines, remote X and some drivers,
/// pages stay blank, so WebKit is told to draw them another way, unless the person already
/// chose. It sets the environment, so it runs before any thread starts.
pub fn choose_page_rendering() {
    const DMABUF: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";
    if std::env::var_os(DMABUF).is_some() {
        return;
    }
    let missing = if !has_dri3() {
        "the X server has no DRI3"
    } else if !has_render_node() {
        "there is no render node to draw with"
    } else {
        return;
    };
    std::env::set_var(DMABUF, "1");
    eprintln!("sikemux: {missing}, so web pages are drawn without DMA-BUF buffers");
}

fn has_dri3() -> bool {
    // SAFETY: a connection of its own, closed before returning; X fills the three numbers.
    unsafe {
        let display = xlib::XOpenDisplay(std::ptr::null());
        if display.is_null() {
            return false;
        }
        let (mut opcode, mut event, mut error) = (0, 0, 0);
        let found = xlib::XQueryExtension(
            display,
            c"DRI3".as_ptr(),
            &mut opcode,
            &mut event,
            &mut error,
        ) != 0;
        xlib::XCloseDisplay(display);
        found
    }
}

fn has_render_node() -> bool {
    let Ok(nodes) = std::fs::read_dir("/dev/dri") else {
        return false;
    };
    nodes.flatten().any(|node| {
        node.file_name().to_string_lossy().starts_with("renderD")
            && std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(node.path())
                .is_ok()
    })
}

/// Starts the thread GTK runs on, once, and says whether GTK could start.
pub(crate) fn gtk_thread() -> Result<(), String> {
    static STARTED: OnceLock<Result<(), String>> = OnceLock::new();
    STARTED
        .get_or_init(|| {
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::Builder::new()
                .name("gtk".into())
                .spawn(move || {
                    let context = glib::MainContext::default();
                    let Ok(_owned) = context.acquire() else {
                        let _ = sender.send(Err("GTK's main context is taken".to_string()));
                        return;
                    };
                    gtk::gdk::set_allowed_backends("x11");
                    let started = gtk::init().map_err(|error| error.to_string());
                    let ok = started.is_ok();
                    let _ = sender.send(started);
                    if ok {
                        glib::MainLoop::new(Some(&context), false).run();
                    }
                })
                .map_err(|error| error.to_string())?;
            receiver
                .recv()
                .map_err(|_| "GTK did not start".to_string())?
        })
        .clone()
}

pub(crate) fn on_gtk(work: impl FnOnce() + Send + 'static) {
    let context = glib::MainContext::default();
    if context.is_owner() {
        work();
    } else {
        context.invoke(work);
    }
}

/// A window by its X11 id, as wry takes a parent.
pub(crate) struct X11Parent(pub(crate) c_ulong);

impl HasWindowHandle for X11Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let handle = RawWindowHandle::Xlib(XlibWindowHandle::new(self.0));
        // SAFETY: the window outlives the call that borrows it to make a page.
        Ok(unsafe { WindowHandle::borrow_raw(handle) })
    }
}

impl Tab {
    /// Adds the tab's page to `parent`, hidden and parked off screen, and starts loading
    /// `spec.url`. Waits for the GTK thread.
    pub fn open(
        parent: ParentView,
        spec: TabSpec<'_>,
        events: Arc<dyn TabEvents>,
    ) -> Result<Tab, String> {
        gtk_thread()?;
        let id: Arc<str> = spec.tab_id.into();
        let (agent_id, tab_id) = (spec.agent_id.to_owned(), id.to_string());
        let (url, user_agent, size) = (spec.url.to_owned(), spec.user_agent.to_owned(), spec.size);
        let window = NonZeroU32::new(parent.view as usize as u32)
            .ok_or("the window has no X11 id")?
            .get();
        let make = move || -> Result<Held, String> {
            let (load, title, popup, started, finished) = (
                events.clone(),
                events.clone(),
                events.clone(),
                events.clone(),
                events.clone(),
            );
            let attached = events.clone();
            let clip = Clip::new(c_ulong::from(window), parent.ground)?;
            clip.place(PARKED, PARKED, size.0, size.1);
            let view = wry::WebViewBuilder::new()
                .with_url(&url)
                .with_user_agent(&user_agent)
                .with_initialization_script(crate::RECORDER_SCRIPT)
                .with_focused(false)
                .with_visible(false)
                .with_hotkeys_zoom(true)
                .with_bounds(wry::Rect {
                    position: wry::dpi::LogicalPosition::new(0., 0.).into(),
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
                    popup.new_window(url, None);
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
                })
                .build_as_child(&X11Parent(clip.window))
                .map_err(|error| error.to_string())?;
            // wry sizes a new child window by the screen's DPI over 96, but every later
            // placement by GTK's own scale, so the page is sized once more the later way.
            let _ = view.set_bounds(wry::Rect {
                position: wry::dpi::LogicalPosition::new(0., 0.).into(),
                size: wry::dpi::LogicalSize::new(size.0, size.1).into(),
            });
            crate::linux::page::attach(&view.webview(), &agent_id, &tab_id, attached);
            Ok(Held::new(Page::new(view, clip, parent.ground)))
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        on_gtk(move || {
            let _ = sender.send(make());
        });
        let held = receiver
            .recv()
            .map_err(|_| "the window cannot take a browser tab".to_string())??;
        Ok(Tab {
            id,
            view: Arc::new(held),
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    fn with(&self, act: impl FnOnce(&wry::WebView) + Send + 'static) {
        self.view.with(move |page| act(&page.view));
    }

    /// Runs `act` on the page on the GTK thread, at once when called there.
    pub fn run(&self, act: impl FnOnce(&WebView) + Send + 'static) {
        self.with(move |view| act(&view.webview()));
    }

    /// Runs `act` on the page on the GTK thread and hands back its answer.
    pub async fn ask<T: Send + 'static>(
        &self,
        act: impl FnOnce(&WebView) -> T + Send + 'static,
    ) -> Result<T, String> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.run(move |webview| {
            let _ = sender.send(act(webview));
        });
        receiver.await.map_err(|_| "the tab went away".to_string())
    }

    pub fn show(&self) {
        self.view.with(|page| page.show(true));
    }

    pub fn hide(&self) {
        self.view.with(|page| page.show(false));
    }

    /// Takes the page out of its window.
    pub fn close(&self) {
        let view = self.view.clone();
        on_gtk(move || drop(view.0.locked().take()));
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

    /// Puts the page at `frame` (x, y, width, height), top-down in the window, with `sides`
    /// (left, right) of it cut off.
    pub fn set_bounds(&self, frame: (f64, f64, f64, f64), sides: (f64, f64)) {
        let (x, y, width, height) = frame;
        let (left, right) = sides;
        self.view
            .with(move |page| page.place((x + left, y, width - left - right, height), frame));
    }

    /// Runs `script` in the page and hands back its value as JSON, or an empty string when it
    /// has none.
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
