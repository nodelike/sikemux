//! Where there is no browser engine yet, no tab can be made, so nothing here ever runs.
//! These stand in for an engine's page calls so the rest of the browser still builds.

const UNAVAILABLE: &str = "browser tabs need macOS or Linux";

pub(crate) type PageView = ();

pub(crate) fn full_page_jpeg(
    _page: &PageView,
    _height: f64,
    _most: f64,
    done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
) {
    done(Err(UNAVAILABLE.into()));
}

pub(crate) fn snapshot_jpeg(
    _page: &PageView,
    _area: Option<(f64, f64, f64, f64)>,
    done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
) {
    done(Err(UNAVAILABLE.into()));
}

pub(crate) fn still_jpeg(_page: &PageView, done: Box<dyn FnOnce(Option<Vec<u8>>) + Send>) {
    done(None);
}

pub(crate) fn history(_page: &PageView, _delta: i32) {}

pub(crate) fn introduce_as(_page: &PageView, _user_agent: &str) {}
