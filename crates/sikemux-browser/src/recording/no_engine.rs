use std::path::Path;
use std::sync::mpsc;

use super::{Frame, Summary};
use crate::no_engine::{Page, NOT_HERE};

pub(super) enum Picture {}

pub(super) fn extension() -> &'static str {
    "mp4"
}

pub(super) fn size(page: &Page) -> (f64, f64) {
    match *page {}
}

pub(super) fn capture(page: &Page, _width: f64, _done: Box<dyn FnOnce(Option<Picture>) + Send>) {
    match *page {}
}

pub(super) enum Writer {}

impl Writer {
    pub(super) fn open(_path: &Path, _width: usize, _height: usize) -> Result<Self, String> {
        Err(NOT_HERE.into())
    }

    pub(super) fn write(self, _incoming: mpsc::Receiver<Frame>) -> Result<Summary, String> {
        match self {}
    }
}
