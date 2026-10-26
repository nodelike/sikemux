//! Recording where there is no browser engine: no tab exists to film, so nothing here runs.

use std::path::Path;
use std::sync::mpsc;

use super::{Frame, Summary};

pub(super) struct Picture;

pub(super) fn extension() -> &'static str {
    "mp4"
}

pub(super) fn size(_page: &crate::unsupported::PageView) -> (f64, f64) {
    (0.0, 0.0)
}

pub(super) fn capture(
    _page: &crate::unsupported::PageView,
    _width: f64,
    done: Box<dyn FnOnce(Option<Picture>) + Send>,
) {
    done(None);
}

pub(super) struct Writer;

impl Writer {
    pub(super) fn open(_path: &Path, _width: usize, _height: usize) -> Result<Self, String> {
        Err("browser tabs need macOS or Linux".into())
    }

    pub(super) fn write(self, _incoming: mpsc::Receiver<Frame>) -> Result<Summary, String> {
        Err("browser tabs need macOS or Linux".into())
    }
}
