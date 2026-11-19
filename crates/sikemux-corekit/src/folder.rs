//! A folder of the person's own files of one kind, such as themes, each named by its id and read
//! again whenever the folder changes. A file that stops reading keeps what it last gave, with
//! its problem beside it.

use std::collections::BTreeMap;
use std::path::Path;

/// The largest file a folder reads, so a stray file cannot fill memory.
pub const MOST_FILE_BYTES: u64 = 256 * 1024;

/// What one file gave, by the file's name without its extension.
#[derive(Clone, Debug)]
pub struct Folder<T> {
    files: BTreeMap<String, File<T>>,
}

#[derive(Clone, Debug)]
struct File<T> {
    kept: Option<T>,
    problem: Option<String>,
}

impl<T> Default for Folder<T> {
    fn default() -> Self {
        Self {
            files: BTreeMap::new(),
        }
    }
}

impl<T> Folder<T> {
    /// Reads every file in `dir` ending in `extension` (any file when it is empty) with `read`,
    /// which is given the file's name without the extension and its text. A file that fails keeps
    /// what it last gave; a file gone takes it away. `read` may answer notes beside a value, such
    /// as what it clamped.
    pub fn reload(
        &mut self,
        dir: &Path,
        extension: &str,
        mut read: impl FnMut(&str, &str) -> Result<(T, Vec<String>), String>,
    ) {
        let mut files = BTreeMap::new();
        for (name, text) in texts(dir, extension) {
            let mut file = self.files.remove(&name).unwrap_or(File {
                kept: None,
                problem: None,
            });
            match text.and_then(|text| read(&name, &text)) {
                Ok((value, notes)) => {
                    file.kept = Some(value);
                    file.problem = (!notes.is_empty()).then(|| notes.join("; "));
                }
                Err(problem) => file.problem = Some(problem),
            }
            files.insert(name, file);
        }
        self.files = files;
    }

    /// Each file's last good value, by name.
    pub fn kept(&self) -> impl Iterator<Item = (&str, &T)> {
        (self.files.iter()).filter_map(|(name, file)| Some((name.as_str(), file.kept.as_ref()?)))
    }

    /// Each file's problem, by name, for the files that have one.
    pub fn problems(&self) -> impl Iterator<Item = (&str, &str)> {
        (self.files.iter())
            .filter_map(|(name, file)| Some((name.as_str(), file.problem.as_deref()?)))
    }

    pub fn problem(&self, name: &str) -> Option<&str> {
        self.files.get(name)?.problem.as_deref()
    }
}

/// The text of each regular file in `dir` with `extension`, by name, or why it cannot be read.
/// A symlink is followed, so a folder kept elsewhere and linked in reads as well.
fn texts(dir: &Path, extension: &str) -> Vec<(String, Result<String, String>)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut texts = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if file_name.starts_with('.') {
            continue;
        }
        let name = match extension {
            "" => file_name,
            _ => match path.extension().and_then(|found| found.to_str()) {
                Some(found) if found == extension => file_name
                    .strip_suffix(extension)
                    .and_then(|stem| stem.strip_suffix('.'))
                    .unwrap_or(file_name),
                _ => continue,
            },
        };
        let text = match std::fs::metadata(&path) {
            Ok(meta) if !meta.is_file() => continue,
            Ok(meta) if meta.len() > MOST_FILE_BYTES => {
                Err(format!("the file is over {} KB", MOST_FILE_BYTES / 1024))
            }
            Ok(_) => std::fs::read_to_string(&path).map_err(|error| error.to_string()),
            Err(error) => Err(error.to_string()),
        };
        texts.push((name.to_owned(), text));
    }
    texts
}

/// JSON with `//` and `/* */` comments and trailing commas, made plain JSON. Each comment and
/// dropped comma becomes spaces, so a parse error's line and column still point into the file.
pub fn without_comments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = bytes.to_vec();
    let mut at = 0;
    let mut in_string = false;
    let mut last_comma: Option<usize> = None;
    while let Some(&byte) = bytes.get(at) {
        if in_string {
            match byte {
                b'\\' => at += 1,
                b'"' => in_string = false,
                _ => {}
            }
            at += 1;
            continue;
        }
        match (byte, bytes.get(at + 1)) {
            (b'"', _) => {
                in_string = true;
                last_comma = None;
            }
            (b'/', Some(b'/')) => {
                while let Some(slot) = out.get_mut(at).filter(|slot| **slot != b'\n') {
                    *slot = b' ';
                    at += 1;
                }
                continue;
            }
            (b'/', Some(b'*')) => {
                let end = (text.get(at + 2..).and_then(|rest| rest.find("*/")))
                    .map_or(bytes.len(), |end| at + 2 + end + 2);
                for blank in out.iter_mut().take(end).skip(at) {
                    if *blank != b'\n' {
                        *blank = b' ';
                    }
                }
                at = end;
                continue;
            }
            (b',', _) => last_comma = Some(at),
            (b'}' | b']', _) => {
                if let Some(comma) = last_comma.take().and_then(|comma| out.get_mut(comma)) {
                    *comma = b' ';
                }
            }
            (byte, _) if byte.is_ascii_whitespace() => {}
            _ => last_comma = None,
        }
        at += 1;
    }
    String::from_utf8(out).unwrap_or_default()
}
