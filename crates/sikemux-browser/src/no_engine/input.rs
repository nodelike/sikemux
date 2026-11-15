use super::Page;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mouse {
    Move,
    Down,
    Drag,
    Up,
}

pub fn mouse(page: &Page, _kind: Mouse, _x: f64, _y: f64, _clicks: isize) -> Result<(), String> {
    match *page {}
}

pub fn key(page: &Page, _name: &str) -> Result<(), String> {
    match *page {}
}

pub fn insert_text(page: &Page, _text: &str) -> Result<(), String> {
    match *page {}
}

pub fn probe_responsiveness(page: &Page) -> Result<(), String> {
    match *page {}
}
