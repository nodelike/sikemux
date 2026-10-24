//! A streaming chat view for GPUI.
//!
//! - [`Markdown`] holds one message's blocks and applies the patches its text
//!   grows by, so only the blocks a patch reaches are laid out again.
//! - [`Pen`] draws those blocks, and lets a transcript select and copy their text.
//! - [`Transcript`] holds a chat's rows by stable id and draws them in a
//!   windowed list anchored to the bottom. It applies the changes pushed to it
//!   (put, update, drop from, keep, prepend a page) once per frame, asks for
//!   earlier pages as the reader nears the top, and selects and copies text.

#[cfg(feature = "counts")]
pub mod counts;
mod draw;
mod find;
mod markdown;
mod select;
mod transcript;

pub use draw::{
    CodeColours, CodeRuns, DrawFence, DrawImage, Fence, FileIcon, FileKind, FileOf, FilePress,
    OnFile, OnLink, OnTask, Pen, Style,
};
pub use find::{FoundChanged, Query};
pub use markdown::{Kind, Markdown, Marks, Node, Piece, Rich, Table};
pub use transcript::{init, AwayChanged, Change, Copy, EmptyChanged, Row, RowsChanged, Transcript};
