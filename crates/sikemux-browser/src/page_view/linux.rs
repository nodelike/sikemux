use std::os::raw::c_ulong;
use std::sync::Arc;

use gtk::glib::object::Cast;
use javascriptcore::ValueExt;
use webkit2gtk::{
    NavigationPolicyDecision, NavigationPolicyDecisionExt, NavigationType, PolicyDecisionExt,
    PolicyDecisionType, URIRequestExt, UserContentInjectedFrames, UserContentManagerExt,
    UserScript, UserScriptInjectionTime, WebViewExt,
};
use wry::WebViewExtUnix;

use super::{PageEvent, Room, BOOTSTRAP};
use crate::linux::{gtk_thread, on_gtk, Clip, Held, Page, X11Parent};

/// The page's view in a clip window of its own, which the app places, and the address
/// its text is read from.
pub struct PageView(Arc<Held>, Option<String>);

/// What the page's script reports, heard on the GTK thread.
type Events = Arc<dyn Fn(PageEvent) + Send + Sync>;

impl PageView {
    /// Shows `html` in `room` in the app's window `parent`, out of sight until placed and on
    /// `ground` until its first frame, and tells `events` what the page's script reports. A
    /// reply's page has already been given the app's theme.
    pub fn new(
        parent: u32,
        html: &str,
        room: Room,
        ground: [u8; 3],
        events: impl Fn(PageEvent) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        gtk_thread()?;
        let (fills, base) = match room {
            Room::Grows => (false, None),
            Room::Fills(path) => (
                true,
                path.and_then(|path| url::Url::from_file_path(path).ok())
                    .map(String::from),
            ),
        };
        let (html, events): (String, Events) = (html.to_owned(), Arc::new(events));
        let (sender, receiver) = std::sync::mpsc::channel();
        let loads = base.clone();
        on_gtk(move || {
            let opened = open(c_ulong::from(parent), &html, fills, loads, ground, events);
            let _ = sender.send(opened.map(Held::new));
        });
        let held = receiver
            .recv()
            .map_err(|_| "the window cannot take a page".to_string())??;
        Ok(Self(Arc::new(held), base))
    }

    /// Shows `html` in place of what the page showed.
    pub fn show(&self, html: &str) {
        let (html, base) = (html.to_owned(), self.1.clone());
        self.0.with(move |page| {
            page.view.webview().load_html(&html, base.as_deref());
        });
    }

    /// Shows the page at `bounds`, cut to the part in `visible`, both (x, y, width, height)
    /// top-down in the window, or puts it out of sight on `None`.
    pub fn place(&self, frame: Option<((f64, f64, f64, f64), (f64, f64, f64, f64))>) {
        self.0.with(move |page| match frame {
            Some((bounds, visible)) => {
                page.place(visible, bounds);
                page.show(true);
            }
            None => page.show(false),
        });
    }

    /// Runs `script` in the page, as when the window's theme changes under it.
    pub fn run_script(&self, script: &str) {
        let script = script.to_owned();
        self.0.with(move |page| {
            page.view.webview().evaluate_javascript(
                &script,
                None,
                None,
                None::<&gtk::gio::Cancellable>,
                |_| {},
            );
        });
    }
}

/// Makes the page's view with no storage, loads `html` from `base` once the page's script can
/// report, and lets the page go nowhere: a link the person clicks is reported instead.
fn open(
    parent: c_ulong,
    html: &str,
    fills: bool,
    base: Option<String>,
    ground: [u8; 3],
    events: Events,
) -> Result<Page, String> {
    let clip = Clip::new(parent, Some(ground))?;
    let view = wry::WebViewBuilder::new()
        .with_incognito(true)
        .with_focused(false)
        .with_visible(false)
        .with_bounds(wry::Rect {
            position: wry::dpi::LogicalPosition::new(0., 0.).into(),
            size: wry::dpi::LogicalSize::new(1., 1.).into(),
        })
        .build_as_child(&X11Parent(clip.window))
        .map_err(|error| error.to_string())?;
    let webview = view.webview();
    let scripts = webview
        .user_content_manager()
        .ok_or("the page has no script manager")?;
    if !fills {
        scripts.add_script(&UserScript::new(
            BOOTSTRAP,
            UserContentInjectedFrames::TopFrame,
            UserScriptInjectionTime::Start,
            &[],
            &[],
        ));
    }
    scripts.register_script_message_handler("sikemux");
    let heard = events.clone();
    scripts.connect_script_message_received(Some("sikemux"), move |_, result| {
        let text = result.js_value().map(|value| value.to_str());
        if let Some(event) = text.as_deref().and_then(|text| PageEvent::read(text)) {
            heard(event);
        }
    });
    let loaded = super::loads_as(base.as_deref()).to_owned();
    webview.connect_decide_policy(move |_, decision, kind| {
        if kind != PolicyDecisionType::NavigationAction {
            return false;
        }
        let Some(decision) = decision.downcast_ref::<NavigationPolicyDecision>() else {
            return false;
        };
        let Some(action) = decision.navigation_action() else {
            return false;
        };
        let url = action
            .request()
            .and_then(|request| request.uri())
            .map(|url| url.to_string())
            .unwrap_or_default();
        let kind = action.navigation_type();
        if kind == NavigationType::LinkClicked && url.starts_with("http") {
            events(PageEvent::Link(url.clone()));
        }
        if super::stays(&url, &loaded, kind == NavigationType::Other) {
            decision.use_();
        } else {
            decision.ignore();
        }
        true
    });
    webview.load_html(html, base.as_deref());
    Ok(Page::new(view, clip, Some(ground)))
}
