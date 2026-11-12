//! A web page in a view of its own, placed where its element is laid out: an agent's page in
//! its reply, or an HTML file as it reads.

use gpui_kit::{App, Window};
use gpui_native_view::NativeSurface;
pub(crate) use sikemux_browser::page_view::{PageEvent, Room};

pub(crate) type Events = tokio::sync::mpsc::UnboundedSender<PageEvent>;

/// A page's view and the surface that places it.
#[derive(Clone)]
pub(crate) struct View {
    pub(crate) surface: NativeSurface,
    page: std::rc::Rc<sikemux_browser::page_view::PageView>,
}

impl View {
    pub(crate) fn show(&self, html: &str) {
        self.page.show(html);
    }

    /// A still of what the page shows now, as a JPEG; none where WebKit cannot make one.
    pub(crate) fn still(&self, done: Box<dyn FnOnce(Option<Vec<u8>>) + Send>) {
        #[cfg(target_os = "macos")]
        self.page.still(done);
        #[cfg(not(target_os = "macos"))]
        done(None);
    }

    /// Lets the page fill its room and scroll itself, or puts it back to scrolling with its reply.
    pub(crate) fn fill(&self, fills: bool) {
        #[cfg(target_os = "macos")]
        self.page.fill(fills);
        #[cfg(not(target_os = "macos"))]
        let _ = fills;
    }

    /// Runs `script` in the page, with no answer waited for.
    pub(crate) fn run(&self, script: &str) {
        self.page.run_script(script);
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn open(
    html: &str,
    room: Room,
    events: Events,
    window: &Window,
    _: &App,
) -> Option<View> {
    use sikemux_browser::page_view::PageView;
    let mtm = objc2::MainThreadMarker::new()?;
    let page = std::rc::Rc::new(PageView::new(mtm, html, room, move |event| {
        let _ = events.send(event);
    }));
    let view = gpui_native_view::OwnedView::new(page.view(), window)?;
    Some(View {
        surface: NativeSurface::new(Shows {
            _page: page.clone(),
            view,
            parks: matches!(room, Room::App),
        }),
        page,
    })
}

#[cfg(target_os = "linux")]
pub(crate) fn open(
    html: &str,
    room: Room,
    events: Events,
    window: &Window,
    cx: &App,
) -> Option<View> {
    let parent = gpui_native_view::window_x11(window)?;
    let ground = match room {
        Room::Grows | Room::App | Room::Panel => {
            crate::theme::page_ground_rgb(crate::theme::Tokens::get(cx))
        }
        Room::Fills(_) => [255; 3],
    };
    let page =
        sikemux_browser::page_view::PageView::new(parent, html, room, ground, move |event| {
            let _ = events.send(event);
        });
    match page {
        Ok(page) => {
            let page = std::rc::Rc::new(page);
            Some(View {
                surface: NativeSurface::new(Shows(page.clone())),
                page,
            })
        }
        Err(error) => {
            eprintln!("sikemux: a page could not be shown: {error}");
            None
        }
    }
}

/// The page's view placed in its element, kept with the page that fills it.
#[cfg(target_os = "macos")]
struct Shows {
    _page: std::rc::Rc<sikemux_browser::page_view::PageView>,
    view: gpui_native_view::OwnedView,
    /// Out of sight it shrinks to nothing rather than hiding: WebKit holds the main thread for up
    /// to a quarter second when a hidden view shows again while its page is busy, as an app's
    /// view is each time it is lent to another app.
    parks: bool,
}

#[cfg(target_os = "macos")]
impl gpui_native_view::Surface for Shows {
    fn place(&self, frame: Option<gpui_native_view::Frame>) {
        let parked = gpui_native_view::Frame {
            bounds: gpui_kit::Bounds::default(),
            visible: gpui_kit::Bounds::default(),
        };
        self.view
            .place(frame.or_else(|| self.parks.then_some(parked)));
    }

    fn responder(&self) -> Option<gpui_native_view::Retained<gpui_native_view::NSView>> {
        self.view.responder()
    }
}

/// The page's own window, placed in its element and cut to the part of it that shows.
#[cfg(target_os = "linux")]
struct Shows(std::rc::Rc<sikemux_browser::page_view::PageView>);

#[cfg(target_os = "linux")]
impl gpui_native_view::Surface for Shows {
    fn place(&self, frame: Option<gpui_native_view::Frame>) {
        let rect = |bounds: gpui_kit::Bounds<gpui_kit::Pixels>| {
            (
                f64::from(bounds.left()),
                f64::from(bounds.top()),
                f64::from(bounds.size.width),
                f64::from(bounds.size.height),
            )
        };
        self.0
            .place(frame.map(|frame| (rect(frame.bounds), rect(frame.visible))));
    }
}
