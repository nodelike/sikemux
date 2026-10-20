//! What a project can run: its `sikemux.json`, the tasks its build files
//! suggest, the Java they need, what stops them starting, and the test results
//! they leave behind.

pub mod check;
pub mod config;
pub mod detect;
pub mod folders;
pub mod java;
pub mod languages;
pub mod results;
pub mod tools;

use std::io::Read;
use std::path::Path;

const MAX_TEXT_BYTES: u64 = 2 * 1024 * 1024;

/// A text file's contents, or none when it is missing, unreadable, too big or
/// binary.
pub fn read_text(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > MAX_TEXT_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_TEXT_BYTES).read_to_end(&mut bytes).ok()?;
    if bytes.iter().take(8192).any(|byte| *byte == 0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}
