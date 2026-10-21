//! What the file tree and the editor do on disk, in one place: the core of
//! the host that holds a project's copy serves it to every client as the
//! `files.*` methods, and the app on the same host calls it directly. What
//! moved on disk reaches clients as the project watcher's `git` change
//! notices, which name the paths.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sikemux_process::sync::Locked;

/// Folders one `files.list` may name: every folder the person has open in a tree.
const MAX_DIRS: usize = 512;
/// Entries one `files.list` answers with across its folders; a folder cut
/// short says how many it holds, and the client asks for the rest.
const MAX_ENTRIES: usize = 20_000;
const MAX_KINDS: usize = 256;
/// The largest file the editor opens.
pub const MAX_TEXT_BYTES: u64 = 16 * 1024 * 1024;
/// The most of a file one read answers with over a connection, so a large
/// file never holds up the other replies on it.
pub const PIECE_BYTES: u64 = 1024 * 1024;
const MAX_UPLOADS: usize = 16;
const UPLOAD_IDLE: Duration = Duration::from_secs(60);

/// What went wrong, starting `fs: `, `file-conflict: `, `not-text: ` or
/// `invalid argument: ` so a client can tell which.
#[derive(Debug)]
pub struct Error(String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self(format!("fs: {error}"))
    }
}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Self(message)
    }
}

impl From<&str> for Error {
    fn from(message: &str) -> Self {
        Self(message.to_owned())
    }
}

type Result<T> = std::result::Result<T, Error>;

/// Runs `files.<method>` with `params`, reading at most `piece` bytes of a
/// file at once. Blocks on the disk.
pub fn run(method: &str, params: Value, piece: u64) -> Result<Value> {
    fn answer(value: impl Serialize) -> Result<Value> {
        serde_json::to_value(value).map_err(|error| format!("json: {error}").into())
    }
    match method {
        "files.list" => answer(list(parse(params)?)?),
        "files.kinds" => answer(kinds(parse(params)?)?),
        "files.read" => answer(read(parse(params)?, piece)?),
        "files.write" => answer(write(parse(params)?)?),
        "files.create" => answer(create(parse(params)?)?),
        "files.rename" => answer(rename(parse(params)?)?),
        "files.delete" => answer(delete(parse(params)?)?),
        _ => Err(format!("this host does not know {method}").into()),
    }
}

/// Whether `answer`, to `method`, is a save that landed.
pub fn saved(method: &str, answer: &Value) -> bool {
    method == "files.write" && answer.get("version").is_some()
}

fn parse<T: for<'de> Deserialize<'de>>(params: Value) -> Result<T> {
    serde_json::from_value(params)
        .map_err(|error| format!("invalid argument: the call's params do not fit: {error}").into())
}

fn absolute(path: &str) -> Result<&Path> {
    let path = Path::new(path);
    if path.is_absolute() && !path.as_os_str().is_empty() {
        Ok(path)
    } else {
        Err(format!(
            "invalid argument: {} is not an absolute path",
            path.display()
        )
        .into())
    }
}

#[derive(Deserialize)]
struct ListParams {
    dirs: Vec<String>,
    /// Where in each folder, sorted as answered, to start.
    #[serde(default)]
    from: usize,
}

/// One folder's entries, folders first then files, both by name. `total`
/// counts them all, so a client knows when `entries` stops short.
#[derive(Serialize)]
struct Listing {
    dir: String,
    entries: Vec<Entry>,
    total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
struct Entry {
    name: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    dir: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    ignored: bool,
}

fn list(params: ListParams) -> Result<Vec<Listing>> {
    if params.dirs.len() > MAX_DIRS {
        return Err(
            format!("invalid argument: one listing may name at most {MAX_DIRS} folders").into(),
        );
    }
    let mut budget = MAX_ENTRIES;
    let mut listings: Vec<Listing> = params
        .dirs
        .into_iter()
        .map(|dir| match read_dir_sorted(&dir) {
            Ok(mut entries) => {
                let total = entries.len();
                let start = params.from.min(total);
                let end = total.min(start + budget);
                budget -= end - start;
                entries.truncate(end);
                entries.drain(..start);
                Listing {
                    dir,
                    entries,
                    total,
                    error: None,
                }
            }
            Err(error) => Listing {
                dir,
                entries: Vec::new(),
                total: 0,
                error: Some(error.to_string()),
            },
        })
        .collect();
    mark_ignored(&mut listings);
    Ok(listings)
}

/// `.git` is left out; other dotfiles stay, since the person edits them.
fn read_dir_sorted(dir: &str) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(absolute(dir)?)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".git" {
            continue;
        }
        let dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        entries.push(Entry {
            name,
            dir,
            ignored: false,
        });
    }
    entries.sort_by_cached_key(|entry| (!entry.dir, entry.name.to_lowercase()));
    Ok(entries)
}

fn repo_root(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
        .map(Path::to_path_buf)
}

/// Flags what git ignores, asking each repository once for all its folders.
fn mark_ignored(listings: &mut [Listing]) {
    let joined = |listing: &Listing, entry: &Entry| {
        Path::new(&listing.dir)
            .join(&entry.name)
            .to_string_lossy()
            .into_owned()
    };
    let mut by_repo: HashMap<PathBuf, Vec<String>> = HashMap::new();
    for listing in listings.iter() {
        if listing.entries.is_empty() {
            continue;
        }
        if let Some(root) = repo_root(Path::new(&listing.dir)) {
            let paths = by_repo.entry(root).or_default();
            paths.extend(listing.entries.iter().map(|entry| joined(listing, entry)));
        }
    }
    let ignored: HashSet<String> = by_repo
        .into_iter()
        .flat_map(|(root, paths)| crate::git::ignored_paths(&root.to_string_lossy(), &paths))
        .collect();
    if ignored.is_empty() {
        return;
    }
    for listing in listings.iter_mut() {
        let dir = Path::new(&listing.dir).to_owned();
        for entry in &mut listing.entries {
            entry.ignored = ignored.contains(dir.join(&entry.name).to_string_lossy().as_ref());
        }
    }
}

#[derive(Deserialize)]
struct KindsParams {
    paths: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    File,
    Dir,
}

/// Which paths exist, and which of those are folders.
fn kinds(params: KindsParams) -> Result<Vec<Option<Kind>>> {
    if params.paths.len() > MAX_KINDS {
        return Err(format!("invalid argument: one ask may name at most {MAX_KINDS} paths").into());
    }
    Ok(params
        .paths
        .iter()
        .map(|path| {
            let meta = fs::metadata(absolute(path).ok()?).ok()?;
            Some(if meta.is_dir() { Kind::Dir } else { Kind::File })
        })
        .collect())
}

#[derive(Deserialize)]
struct ReadParams {
    path: String,
    /// The byte to start at, where the last piece ended.
    #[serde(default)]
    at: u64,
}

/// A piece of a text file. `next` is the byte after it; the piece that
/// reaches `size` carries the whole file's `version`, which a save names to
/// say what it replaces. `stamp` changes when the file does, so a client
/// reading it in pieces starts again when it moves under it.
#[derive(Serialize)]
struct Piece {
    text: String,
    next: u64,
    size: u64,
    stamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
}

fn read(params: ReadParams, piece: u64) -> Result<Piece> {
    let path = absolute(&params.path)?;
    let mut file = open_regular(path)?;
    let meta = file.metadata()?;
    let size = meta.len();
    if size > MAX_TEXT_BYTES {
        return Err(format!(
            "fs: {} exceeds the {} MB read limit",
            path.display(),
            MAX_TEXT_BYTES / 1024 / 1024
        )
        .into());
    }
    let at = params.at.min(size);
    file.seek(SeekFrom::Start(at))?;
    let mut bytes = Vec::with_capacity((size - at).min(piece) as usize);
    file.take(piece).read_to_end(&mut bytes)?;
    let reached_end = at + bytes.len() as u64 >= size;
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => {
            let utf8 = error.utf8_error();
            if reached_end || utf8.error_len().is_some() {
                return Err(not_text(path));
            }
            let mut bytes = error.into_bytes();
            bytes.truncate(utf8.valid_up_to());
            String::from_utf8(bytes).map_err(|_| not_text(path))?
        }
    };
    let next = at + text.len() as u64;
    let version = if !reached_end {
        None
    } else if at == 0 {
        Some(content_version(text.as_bytes()))
    } else {
        Some(content_version(&read_bounded(path)?))
    };
    Ok(Piece {
        text,
        next,
        size,
        stamp: stamp(&meta),
        version,
    })
}

fn not_text(path: &Path) -> Error {
    format!("not-text: {}", path.display()).into()
}

fn conflict(message: String) -> Error {
    format!("file-conflict: {message}").into()
}

fn stamp(meta: &fs::Metadata) -> String {
    use std::os::unix::fs::MetadataExt;
    format!(
        "{}-{}.{}-{}",
        meta.len(),
        meta.mtime(),
        meta.mtime_nsec(),
        meta.ino()
    )
}

fn content_version(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Opening without blocking turns a named pipe away instead of hanging until
/// something writes to it.
fn open_regular(path: &Path) -> Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(format!("fs: {} is not a regular file", path.display()).into());
    }
    Ok(file)
}

fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    let file = open_regular(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_TEXT_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_TEXT_BYTES {
        return Err(format!("fs: {} exceeds the read limit", path.display()).into());
    }
    Ok(bytes)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WriteParams {
    path: String,
    text: String,
    /// The version the client read; the save is refused when the file on
    /// disk is no longer it.
    version: Option<String>,
    /// Refuse to replace anything already at `path`.
    #[serde(default)]
    new: bool,
    /// The save this piece continues.
    upload: Option<String>,
    /// More pieces follow this one.
    #[serde(default)]
    more: bool,
}

/// A finished save's `version`, or the `upload` the next piece names.
#[derive(Serialize)]
struct Written {
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    upload: Option<String>,
}

/// The earlier pieces of a save that arrives in several, kept in the host's
/// temporary folder, so a save that never finishes leaves nothing in the project.
struct Upload {
    path: PathBuf,
    staged: tempfile::NamedTempFile,
    hash: Sha256,
    size: u64,
    touched: Instant,
}

fn uploads() -> &'static Mutex<HashMap<String, Upload>> {
    static UPLOADS: OnceLock<Mutex<HashMap<String, Upload>>> = OnceLock::new();
    UPLOADS.get_or_init(Default::default)
}

fn write(params: WriteParams) -> Result<Written> {
    let path = absolute(&params.path)?.to_owned();
    if let Some(version) = &params.version {
        if version.len() != 64 || !version.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("invalid argument: version must be a SHA-256 hex digest".into());
        }
    }
    let earlier = {
        let mut uploads = uploads().locked();
        uploads.retain(|_, upload| upload.touched.elapsed() < UPLOAD_IDLE);
        match &params.upload {
            Some(id) => Some(
                uploads
                    .remove(id)
                    .filter(|upload| upload.path == path)
                    .ok_or("invalid argument: that save timed out; save again")?,
            ),
            None => None,
        }
    };
    let text = params.text.as_bytes();
    let size = earlier.as_ref().map_or(0, |upload| upload.size) + text.len() as u64;
    if size > MAX_TEXT_BYTES {
        return Err(format!("fs: {} exceeds the save limit", path.display()).into());
    }
    let mut upload = match earlier {
        Some(upload) => upload,
        None if params.more => Upload {
            path: path.clone(),
            staged: tempfile::NamedTempFile::new()?,
            hash: Sha256::new(),
            size: 0,
            touched: Instant::now(),
        },
        None => return save(&path, None, Sha256::new(), text, &params),
    };
    if params.more {
        upload.staged.write_all(text)?;
        upload.hash.update(text);
        upload.size = size;
        upload.touched = Instant::now();
        let id = uuid::Uuid::new_v4().to_string();
        let mut uploads = uploads().locked();
        if uploads.len() >= MAX_UPLOADS {
            return Err("fs: too many saves at once; try again".into());
        }
        uploads.insert(id.clone(), upload);
        return Ok(Written {
            version: None,
            upload: Some(id),
        });
    }
    upload.staged.seek(SeekFrom::Start(0))?;
    save(&path, Some(upload.staged), upload.hash, text, &params)
}

/// Writes the save beside its file, so it lands as one rename, keeping the
/// file's permissions. A symlink is kept by replacing what it points at.
fn save(
    path: &Path,
    staged: Option<tempfile::NamedTempFile>,
    mut hash: Sha256,
    last: &[u8],
    params: &WriteParams,
) -> Result<Written> {
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let parent = target
        .parent()
        .ok_or_else(|| Error::from("invalid argument: that path has no folder"))?;
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    if let Ok(meta) = fs::metadata(&target) {
        temp.as_file().set_permissions(meta.permissions())?;
    }
    if let Some(mut staged) = staged {
        std::io::copy(&mut staged, &mut temp)?;
    }
    temp.write_all(last)?;
    hash.update(last);
    finish(temp, path, params.version.as_deref(), params.new)?;
    Ok(Written {
        version: Some(hex::encode(hash.finalize())),
        upload: None,
    })
}

fn finish(
    temp: tempfile::NamedTempFile,
    path: &Path,
    expected: Option<&str>,
    new: bool,
) -> Result<()> {
    let lock = write_lock_for(path);
    let _guard = lock.locked();
    temp.as_file().flush()?;
    sync_before_replace(temp.as_file())?;
    if new {
        temp.persist_noclobber(path)
            .map_err(|error| match error.error.kind() {
                std::io::ErrorKind::AlreadyExists => {
                    Error::from(format!("fs: already exists: {}", path.display()))
                }
                _ => Error::from(error.error),
            })?;
        return Ok(());
    }
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    if let Some(expected) = expected {
        let gone = || {
            conflict(format!(
                "{} was deleted or moved outside Sikemux",
                path.display()
            ))
        };
        let on_disk = match read_bounded(&target) {
            Ok(bytes) => content_version(&bytes),
            Err(_) if !target.exists() => return Err(gone()),
            Err(error) => return Err(error),
        };
        if on_disk != expected {
            return Err(conflict(format!(
                "{} changed outside Sikemux; reload it before saving",
                path.display()
            )));
        }
    }
    temp.persist(&target)
        .map_err(|error| Error::from(error.error))?;
    Ok(())
}

/// Two saves of one file, from any clients, take turns.
fn write_lock_for(path: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();
    let key = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let mut locks = LOCKS.get_or_init(Default::default).locked();
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    lock
}

/// Gets the new bytes onto the device before the rename that publishes them.
/// On macOS a barrier orders them without the full flush of the drive's cache
/// an ordinary fsync costs on every save.
fn sync_before_replace(file: &fs::File) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: `file` stays open for the call, so its descriptor is valid, and
        // F_BARRIERFSYNC takes no pointer.
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_BARRIERFSYNC) } != -1 {
            return Ok(());
        }
    }
    file.sync_all()?;
    Ok(())
}

#[derive(Deserialize)]
struct CreateParams {
    path: String,
    /// A folder rather than an empty file.
    #[serde(default)]
    dir: bool,
}

/// A new empty file, never over one already there, or a folder, which may
/// already be there. Missing parents are made.
fn create(params: CreateParams) -> Result<()> {
    let path = absolute(&params.path)?;
    if params.dir {
        fs::create_dir_all(path)?;
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::AlreadyExists => {
                Error::from(format!("fs: already exists: {}", path.display()))
            }
            _ => error.into(),
        })?;
    Ok(())
}

#[derive(Deserialize)]
struct RenameParams {
    from: String,
    to: String,
}

/// Moves an entry, never over one already at `to`.
fn rename(params: RenameParams) -> Result<()> {
    use rustix::fs::{renameat_with, RenameFlags, CWD};
    let from = absolute(&params.from)?;
    let to = absolute(&params.to)?;
    if fs::symlink_metadata(from).is_err() {
        return Err(format!("fs: source missing: {}", from.display()).into());
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE).map_err(|error| {
        let error = std::io::Error::from(error);
        match error.kind() {
            std::io::ErrorKind::AlreadyExists => {
                Error::from(format!("fs: destination already exists: {}", to.display()))
            }
            _ => error.into(),
        }
    })
}

#[derive(Deserialize)]
struct PathParams {
    path: String,
}

/// Moves an entry to the host's trash, where the person can get it back.
fn delete(params: PathParams) -> Result<()> {
    let path = absolute(&params.path)?;
    if fs::symlink_metadata(path).is_err() {
        return Err(format!("fs: path missing: {}", path.display()).into());
    }
    trash::delete(path).map_err(|error| Error::from(format!("fs: {error}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(method: &str, params: Value) -> Result<Value> {
        run(method, params, MAX_TEXT_BYTES)
    }

    #[test]
    fn listings_flag_what_git_ignores() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        let status = sikemux_process::user_environment::command("git")
            .args(["init", "-q"])
            .current_dir(repo)
            .status()
            .unwrap();
        assert!(status.success());
        fs::write(repo.join(".gitignore"), "*.log\ndist/\n").unwrap();
        fs::write(repo.join("debug.log"), "").unwrap();
        fs::write(repo.join("main.rs"), "").unwrap();
        fs::create_dir(repo.join("dist")).unwrap();
        fs::write(repo.join("dist/app.js"), "").unwrap();

        let dirs = [repo, &repo.join("dist")].map(|p| p.to_string_lossy().into_owned());
        let listings = call("files.list", json!({ "dirs": dirs })).unwrap();
        let ignored: Vec<(&str, bool)> = listings
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|listing| listing["entries"].as_array().unwrap())
            .map(|entry| (entry["name"].as_str().unwrap(), entry["ignored"] == true))
            .collect();
        assert_eq!(
            ignored,
            [
                ("dist", true),
                (".gitignore", false),
                ("debug.log", true),
                ("main.rs", false),
                ("app.js", true),
            ]
        );
    }

    #[test]
    fn a_versioned_save_replaces_the_file_and_refuses_a_stale_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("note.txt");
        fs::write(&path, b"before").expect("seed");
        let original = content_version(b"before");

        let saved = call(
            "files.write",
            json!({ "path": path, "text": "after", "version": original }),
        )
        .expect("the save must land");
        assert_eq!(saved["version"], content_version(b"after"));
        assert_eq!(fs::read(&path).expect("read back"), b"after");

        let stale = call(
            "files.write",
            json!({ "path": path, "text": "later", "version": original }),
        );
        assert!(
            stale.is_err_and(|error| error.0.starts_with("file-conflict: ")),
            "a stale version must be refused"
        );
        assert_eq!(fs::read(&path).expect("read back"), b"after");
    }

    #[test]
    fn a_save_keeps_the_file_readable_and_keeps_its_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("script.sh");
        fs::write(&path, b"old").expect("seed");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");

        call("files.write", json!({ "path": path, "text": "new" })).expect("save");

        assert_eq!(fs::read(&path).expect("read back"), b"new");
        let mode = fs::metadata(&path).expect("metadata").permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }
}
