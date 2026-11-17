use super::Page;
use crate::BrowserShortcut;

#[derive(Clone, Copy)]
pub enum World {
    Page,
    Helper,
}

pub(crate) fn install_shortcuts(_send: impl Fn(BrowserShortcut) + Send + Sync + 'static) {}

pub fn call_async(
    page: &Page,
    _body: &str,
    _world: World,
    _done: Box<dyn FnOnce(Result<String, String>) + Send>,
) {
    match *page {}
}

pub fn snapshot_jpeg(
    page: &Page,
    _area: Option<(f64, f64, f64, f64)>,
    _done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
) {
    match *page {}
}

pub fn full_page_jpeg(
    page: &Page,
    _height: f64,
    _most: f64,
    _done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
) {
    match *page {}
}

pub fn history(page: &Page, _delta: i32) {
    match *page {}
}

pub fn introduce_as(page: &Page, _agent: &str) {
    match *page {}
}
