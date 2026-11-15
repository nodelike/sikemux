use std::path::Path;
use std::sync::mpsc;

use super::{Frame, Summary};
use crate::windows::{Page, NOT_HERE};

pub(super) enum Picture {}

pub(super) fn extension() -> &'static str {
    "mp4"
}

pub(super) fn size(_page: &Page) -> (f64, f64) {
    (0.0, 0.0)
}

pub(super) fn capture(_page: &Page, _width: f64, done: Box<dyn FnOnce(Option<Picture>) + Send>) {
    done(None);
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
