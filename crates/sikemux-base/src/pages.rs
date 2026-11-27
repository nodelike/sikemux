//! Pages agents show the person: an HTML document an agent writes, kept in the
//! data folder with every local picture and font it names inlined and every video
//! or sound copied into a folder beside it, so it outlives those files. Whatever
//! else it loads comes from the folder it was written in.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::text::one_line;

/// The folder under the data folder pages are kept in.
pub const FOLDER: &str = "pages";

const ZSTD_LEVEL: i32 = 3;
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];
const BLOBS_FOLDER: &str = "blobs";
const BLOB_MARKER: &str = ";sikemux-blob=";
const BLOB_GC_MIN_AGE: Duration = Duration::from_secs(10 * 60);
const COMPACT_MIN_AGE: Duration = Duration::from_secs(30);
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

const MIB: u64 = 1024 * 1024;
const MAX_SOURCE_BYTES: u64 = 2 * MIB;
const MAX_IMAGE_BYTES: u64 = 10 * MIB;
/// Under the 16 MiB a connection to the core carries, so a reply can always fetch the page.
const MAX_PAGE_BYTES: usize = 15 * MIB as usize;
/// Plaintext HTML budget that fits inside the accounts server's 25 MiB sealed base64url limit.
pub const MAX_SHARED_HTML_BYTES: usize = 19_600_000;
const MAX_TITLE_CHARS: usize = 120;
/// Enough of a page to hold its `<title>`, unless a big inlined font or picture comes first.
const HEAD_BYTES: u64 = 64 * 1024;

/// A path names no scheme, so it holds no `:`, except after a Windows drive letter.
const DRIVE: &str = if cfg!(windows) {
    r"(?:[A-Za-z]:[\\/])?"
} else {
    ""
};

static LOCAL_FILE: LazyLock<Regex> = LazyLock::new(|| {
    #[expect(clippy::expect_used, reason = "the pattern is a constant")]
    Regex::new(&format!(
        r#"(["'(])\s*({DRIVE}[^"'()<>\s:]+?\.(?i:png|jpe?g|gif|webp|avif|svg|bmp|ico|woff2?|ttf|otf|mp4|m4v|webm|mov|mp3|m4a|wav|ogg|oga|opus|flac|aac))\s*(["')])"#,
    ))
    .expect("local file pattern")
});

static KEPT_MEDIA: LazyLock<Regex> = LazyLock::new(|| {
    #[expect(clippy::expect_used, reason = "the pattern is a constant")]
    Regex::new(r#"(["'(])\s*([0-9a-fA-F]{32}/[^"'()<>\s]+)\s*(["')])"#).expect("kept media pattern")
});

static SCRIPT_SRC: LazyLock<Regex> = LazyLock::new(|| {
    #[expect(clippy::expect_used, reason = "the pattern is a constant")]
    Regex::new(r#"(?is)<script\b([^>]*?)\bsrc\s*=\s*(?:"([^"]+)"|'([^']+)')([^>]*)>\s*</script>"#)
        .expect("script src pattern")
});

static LINK_TAG: LazyLock<Regex> = LazyLock::new(|| {
    #[expect(clippy::expect_used, reason = "the pattern is a constant")]
    Regex::new(r#"(?is)<link\b([^>]*?)>"#).expect("link tag pattern")
});

static RESOURCE_TAG: LazyLock<Regex> = LazyLock::new(|| {
    #[expect(clippy::expect_used, reason = "the pattern is a constant")]
    Regex::new(r#"(?is)<(video|audio|source|track|img|iframe|object|embed)\b([^>]*?)>"#)
        .expect("resource tag pattern")
});

static ATTR: LazyLock<Regex> = LazyLock::new(|| {
    #[expect(clippy::expect_used, reason = "the pattern is a constant")]
    Regex::new(r#"(?is)\b([a-z][a-z0-9-]*)\s*=\s*(?:"([^"]*)"|'([^']*)')"#)
        .expect("html attribute pattern")
});

static CLOSING_SCRIPT: LazyLock<Regex> = LazyLock::new(|| {
    #[expect(clippy::expect_used, reason = "the pattern is a constant")]
    Regex::new(r#"(?i)</script"#).expect("closing script pattern")
});

static CLOSING_STYLE: LazyLock<Regex> = LazyLock::new(|| {
    #[expect(clippy::expect_used, reason = "the pattern is a constant")]
    Regex::new(r#"(?i)</style"#).expect("closing style pattern")
});

static DATA_BASE64: LazyLock<Regex> = LazyLock::new(|| {
    #[expect(clippy::expect_used, reason = "the pattern is a constant")]
    Regex::new(r"data:([a-zA-Z0-9.+/-]+);base64,([A-Za-z0-9+/=]{1024,})")
        .expect("data base64 pattern")
});

static DATA_BLOB: LazyLock<Regex> = LazyLock::new(|| {
    #[expect(clippy::expect_used, reason = "the pattern is a constant")]
    Regex::new(r"data:([a-zA-Z0-9.+/-]+);sikemux-blob=([0-9a-f]{64})").expect("data blob pattern")
});

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PageRef {
    pub id: String,
    pub title: String,
}

pub fn is_page_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The id of the page kept in `file`, whose name is that id (with `.html` or `.html.zst`).
pub fn page_id_of(file: &Path) -> Option<&str> {
    let name = file.file_name()?.to_str()?;
    let id = name
        .strip_suffix(".html.zst")
        .or_else(|| name.strip_suffix(".html"))
        .or_else(|| file.file_stem()?.to_str())?;
    is_page_id(id).then_some(id)
}

pub fn is_compressed_page(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".html.zst"))
}

pub fn compressed_file(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.html.zst"))
}

pub fn plain_file(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.html"))
}

/// The stored HTML file for page `id`, preferring the zstd-compressed copy over a legacy `.html`.
pub fn page_file(dir: &Path, id: &str) -> Option<PathBuf> {
    if !is_page_id(id) {
        return None;
    }
    let compressed = compressed_file(dir, id);
    if compressed.is_file() {
        return Some(compressed);
    }
    let plain = plain_file(dir, id);
    if plain.is_file() {
        return Some(plain);
    }
    None
}

fn blobs_dir(dir: &Path) -> PathBuf {
    dir.join(BLOBS_FOLDER)
}

fn unique_temp_path(dir: &Path, prefix: &str) -> PathBuf {
    let seq = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.join(format!(".{prefix}.tmp.{}.{seq}", std::process::id()))
}

fn write_blob(dir: &Path, hash: &str, raw: &[u8]) -> std::io::Result<()> {
    let folder = blobs_dir(dir);
    let target = folder.join(hash);
    if target.is_file() {
        return Ok(());
    }
    std::fs::create_dir_all(&folder)?;
    let temp = unique_temp_path(&folder, hash);
    if let Err(error) = std::fs::write(&temp, raw) {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&temp, &target) {
        let _ = std::fs::remove_file(&temp);
        if !target.is_file() {
            return Err(error);
        }
    }
    Ok(())
}

/// Extracts large inlined base64 `data:` payloads into content-addressed raw binary files under
/// `{dir}/blobs/{sha256}` so shared fonts and screenshots are stored once without base64 bloat.
fn dehydrate_html<'a>(dir: &Path, html: &'a str) -> std::io::Result<Cow<'a, str>> {
    use base64::Engine as _;
    if !html.contains(";base64,") {
        return Ok(Cow::Borrowed(html));
    }
    let mut failure: Option<std::io::Error> = None;
    let replaced = DATA_BASE64.replace_all(html, |found: &regex::Captures| {
        if failure.is_some() {
            return found[0].to_owned();
        }
        let mime = &found[1];
        let b64 = &found[2];
        let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(b64) else {
            return found[0].to_owned();
        };
        if base64::engine::general_purpose::STANDARD.encode(&raw) != b64 {
            return found[0].to_owned();
        }
        let hash = hex::encode(Sha256::digest(&raw));
        if let Err(error) = write_blob(dir, &hash, &raw) {
            failure = Some(error);
            return found[0].to_owned();
        }
        format!("data:{mime}{BLOB_MARKER}{hash}")
    });
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(replaced)
}

fn hydrate_html<'a>(dir: Option<&Path>, html: &'a str) -> Cow<'a, str> {
    use base64::Engine as _;
    if !html.contains(BLOB_MARKER) {
        return Cow::Borrowed(html);
    }
    let Some(dir) = dir else {
        return Cow::Borrowed(html);
    };
    let folder = blobs_dir(dir);
    let mut cache: HashMap<String, String> = HashMap::new();
    DATA_BLOB.replace_all(html, |found: &regex::Captures| {
        let mime = &found[1];
        let hash = &found[2];
        if let Some(encoded) = cache.get(hash) {
            return format!("data:{mime};base64,{encoded}");
        }
        let Ok(raw) = std::fs::read(folder.join(hash)) else {
            return found[0].to_owned();
        };
        let encoded = base64::engine::general_purpose::STANDARD.encode(&raw);
        cache.insert(hash.to_owned(), encoded.clone());
        format!("data:{mime};base64,{encoded}")
    })
}

fn read_raw_html_file(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    let decoded = if is_compressed_page(path) || bytes.starts_with(&ZSTD_MAGIC) {
        zstd::stream::decode_all(bytes.as_slice())?
    } else {
        bytes
    };
    String::from_utf8(decoded)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

/// Reads a stored page file (zstd-compressed `.html.zst` or legacy `.html`) and restores any
/// deduplicated inline assets into self-contained HTML.
pub fn read_file_html(path: &Path) -> std::io::Result<String> {
    let html = read_raw_html_file(path)?;
    Ok(hydrate_html(path.parent(), &html).into_owned())
}

/// Reads page `id` from `dir`, handling both `.html.zst` and legacy `.html` files transparently.
pub fn read_html(dir: &Path, id: &str) -> std::io::Result<String> {
    let path = page_file(dir, id)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "page not found"))?;
    read_file_html(&path)
}

/// Writes page `id` into `dir` as a zstd-compressed `.html.zst` with deduplicated binary blobs,
/// atomically replacing any previous version and removing a legacy uncompressed `.html`.
pub fn write_compressed_html(dir: &Path, id: &str, html: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let dehydrated = dehydrate_html(dir, html)?;
    let compressed = zstd::stream::encode_all(dehydrated.as_bytes(), ZSTD_LEVEL)?;
    let target = compressed_file(dir, id);
    let temp = unique_temp_path(dir, id);
    if let Err(error) = std::fs::write(&temp, compressed) {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&temp, &target) {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    let _ = std::fs::remove_file(plain_file(dir, id));
    Ok(target)
}

/// Deletes every file belonging to page `id` in `dir`.
pub fn remove_page_files(dir: &Path, id: &str) {
    let _ = std::fs::remove_file(compressed_file(dir, id));
    let _ = std::fs::remove_file(plain_file(dir, id));
    let _ = std::fs::remove_file(dir.join(format!("{id}.jpg")));
    let _ = std::fs::remove_file(written_in_file(dir, id));
    let _ = std::fs::remove_file(source_file(dir, id));
    let _ = std::fs::remove_dir_all(media_folder(dir, id));
}

/// Compresses legacy uncompressed `{id}.html` files in `dir` into `{id}.html.zst` with
/// content-addressed asset deduplication. Returns how many pages were compacted.
pub fn compact_dir(dir: &Path) -> usize {
    compact_dir_older_than(dir, COMPACT_MIN_AGE)
}

fn compact_dir_older_than(dir: &Path, min_age: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut compacted = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(id) = name.strip_suffix(".html").filter(|id| is_page_id(id)) else {
            continue;
        };
        if compressed_file(dir, id).is_file() {
            let _ = std::fs::remove_file(&path);
            compacted += 1;
            continue;
        }
        if !min_age.is_zero() {
            let too_recent = entry
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age < min_age);
            if too_recent {
                continue;
            }
        }
        let Ok(html) = std::fs::read_to_string(&path) else {
            continue;
        };
        if write_compressed_html(dir, id, &html).is_ok() {
            compacted += 1;
        }
    }
    compacted
}

/// Deletes blobs in `{dir}/blobs/` that no surviving page references and are old enough not to
/// belong to an in-flight write. Returns how many blobs were removed.
pub fn gc_blobs(dir: &Path) -> usize {
    gc_blobs_older_than(dir, BLOB_GC_MIN_AGE)
}

fn gc_blobs_older_than(dir: &Path, min_age: Duration) -> usize {
    let folder = blobs_dir(dir);
    let Ok(blob_entries) = std::fs::read_dir(&folder) else {
        return 0;
    };
    let blobs: Vec<(PathBuf, String, Option<Duration>)> = blob_entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return None;
            }
            let name = path.file_name()?.to_str()?.to_owned();
            if name.starts_with('.') {
                return None;
            }
            let age = entry
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .and_then(|modified| modified.elapsed().ok());
            Some((path, name, age))
        })
        .collect();
    if blobs.is_empty() {
        return 0;
    }

    let Ok(page_entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut referenced = HashSet::new();
    for entry in page_entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let is_page = name
            .strip_suffix(".html.zst")
            .or_else(|| name.strip_suffix(".html"))
            .is_some_and(is_page_id);
        if !is_page {
            continue;
        }
        let Ok(raw_html) = read_raw_html_file(&path) else {
            return 0;
        };
        if raw_html.contains(BLOB_MARKER) {
            for found in DATA_BLOB.captures_iter(&raw_html) {
                referenced.insert(found[2].to_owned());
            }
        }
    }

    let mut removed = 0usize;
    for (path, name, age) in blobs {
        if referenced.contains(&name) {
            continue;
        }
        if !min_age.is_zero() && age.is_none_or(|age| age < min_age) {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/* Elements ahead of `<html>` land in the head the parser makes for them, so
the markup goes straight after the doctype and needs no parsing of the page. */
pub fn insert_head(html: &str, markup: &str) -> String {
    let body = html.trim_start_matches('\u{feff}');
    let rest = body.trim_start();
    let lead = body.len() - rest.len();
    let doctype = (rest.get(..9)).is_some_and(|start| start.eq_ignore_ascii_case("<!doctype"));
    let at = if doctype {
        rest.find('>').map_or(0, |end| lead + end + 1)
    } else {
        0
    };
    let (before, after) = body.split_at_checked(at).unwrap_or(("", body));
    format!("{before}{markup}{after}")
}

fn escape_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Reads the page an agent wrote and inlines every local picture and font it
/// names, by absolute, `~/` or relative path, so the page outlives those files.
/// When `allowed_roots` is given, `source` and every local asset it inlines or
/// copies must canonicalize inside one of those directories.
/// Videos and sounds are too big to inline: with `media` they are copied into
/// that folder and named relative to its parent, and without it left as named.
pub fn build(
    source: &Path,
    media: Option<&Path>,
    allowed_roots: Option<&[&Path]>,
) -> Result<String, String> {
    let canonical_roots: Option<Vec<PathBuf>> = allowed_roots.map(|roots| {
        roots
            .iter()
            .filter_map(|root| sikemux_process::files::real_path(root).ok())
            .collect()
    });
    if let Some(roots) = &canonical_roots {
        let canon_source = sikemux_process::files::real_path(source)
            .map_err(|_| format!("{} does not exist", source.display()))?;
        if !roots.iter().any(|root| canon_source.starts_with(root)) {
            return Err(format!(
                "{} is outside the allowed folder",
                source.display()
            ));
        }
    }
    let size = std::fs::metadata(source)
        .map_err(|_| format!("{} does not exist", source.display()))?
        .len();
    if size > MAX_SOURCE_BYTES {
        return Err(format!(
            "{} is {:.1} MiB; a page can be at most 2 MiB before its pictures are inlined",
            source.display(),
            size as f64 / MIB as f64
        ));
    }
    let html = std::fs::read_to_string(source)
        .map_err(|_| format!("{} is not UTF-8 text", source.display()))?;
    let folder = source.parent().unwrap_or(Path::new("/"));
    let mut inlined = HashMap::<String, String>::new();
    let mut missing = Vec::new();
    let mut failure = None;
    let page = LOCAL_FILE.replace_all(&html, |found: &regex::Captures| {
        let (open, reference, close) = (&found[1], &found[2], &found[3]);
        let paired = (open == "(" && close == ")") || (open != "(" && open == close);
        if !paired || reference.starts_with("//") {
            return found[0].to_owned();
        }
        if let Some(data) = inlined.get(reference) {
            return format!("{open}{data}{close}");
        }
        let path = resolve(folder, reference);
        if let Some(roots) = &canonical_roots {
            match sikemux_process::files::real_path(&path) {
                Ok(canon) if !roots.iter().any(|root| canon.starts_with(root)) => {
                    failure.get_or_insert_with(|| {
                        format!("{reference} is outside the allowed folder")
                    });
                    return found[0].to_owned();
                }
                Ok(_) => {}
                Err(_) => {
                    missing.push(reference.to_owned());
                    return found[0].to_owned();
                }
            }
        }
        let read = match is_media(&path) {
            true => match media {
                Some(media) => copy_media(&path, media, inlined.len()),
                None => return found[0].to_owned(),
            },
            false => image_data(&path),
        };
        match read {
            Ok(Some(data)) => {
                inlined.insert(reference.to_owned(), data.clone());
                format!("{open}{data}{close}")
            }
            Ok(None) => {
                missing.push(reference.to_owned());
                found[0].to_owned()
            }
            Err(error) => {
                failure.get_or_insert(error);
                found[0].to_owned()
            }
        }
    });
    if let Some(error) = failure {
        return Err(error);
    }
    if !missing.is_empty() {
        missing.sort();
        missing.dedup();
        return Err(format!(
            "these files could not be read: {}. Use paths to existing images or fonts, relative to the page or absolute, or remove them",
            missing.join(", ")
        ));
    }
    if page.len() > MAX_PAGE_BYTES {
        return Err(format!(
            "with its pictures and fonts inlined the page is {:.1} MiB; the limit is 15 MiB. Each mention of a file inlines it again, so name a big picture once, and use smaller pictures and fonts",
            page.len() as f64 / MIB as f64
        ));
    }
    Ok(page.into_owned())
}

fn resolve(folder: &Path, reference: &str) -> PathBuf {
    folder.join(sikemux_process::paths::expand_home(reference))
}

fn is_media(path: &Path) -> bool {
    let kind = content_type(path);
    kind.starts_with("video/") || kind.starts_with("audio/")
}

/// Copies the video or sound at `path` into `media`, under a name no other file of the page
/// takes, and gives its address relative to the page.
fn copy_media(path: &Path, media: &Path, index: usize) -> Result<Option<String>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let name = path.file_name().map_or_else(String::new, |name| {
        name.to_string_lossy()
            .replace(['"', '\'', '(', ')', ' ', '#', '?', '%'], "-")
    });
    let name = format!("{index}-{name}");
    std::fs::create_dir_all(media).map_err(|error| format!("could not keep the page: {error}"))?;
    std::fs::copy(path, media.join(&name))
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let folder = media
        .file_name()
        .map(|folder| folder.to_string_lossy())
        .unwrap_or_default();
    Ok(Some(format!("{folder}/{name}")))
}

fn image_data(path: &Path) -> Result<Option<String>, String> {
    let Ok(metadata) = std::fs::metadata(path) else {
        return Ok(None);
    };
    if !metadata.is_file() {
        return Ok(None);
    }
    if metadata.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "{} is {:.1} MiB; each picture or font can be at most 10 MiB",
            path.display(),
            metadata.len() as f64 / MIB as f64
        ));
    }
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    use base64::Engine;
    Ok(Some(format!(
        "data:{};base64,{}",
        content_type(path),
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )))
}

fn content_type(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        _ => crate::http::content_type(path),
    }
}

/// Keeps the page an agent wrote at `source` in `dir` and names it for the
/// chat.
pub fn publish(dir: &Path, source: &Path, title: &str) -> Result<PageRef, String> {
    let title: String = one_line(title).chars().take(MAX_TITLE_CHARS).collect();
    if title.is_empty() {
        return Err("title must be nonempty".into());
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    if let Err(error) = keep(dir, &id, source, &title) {
        let _ = std::fs::remove_dir_all(media_folder(dir, &id));
        return Err(error);
    }
    Ok(PageRef { id, title })
}

/// Keeps page `id` again from the file it was first shown from, so a show by its id has the
/// file as it is now. Whether the page changed; it stays as it was when that file is gone.
pub fn refresh(dir: &Path, id: &str) -> Result<bool, String> {
    let Some(source) = (std::fs::read_to_string(source_file(dir, id)).ok())
        .map(PathBuf::from)
        .filter(|source| source.is_file())
    else {
        return Ok(false);
    };
    let title = title(dir, id).unwrap_or_else(|| "Page".to_owned());
    let was = kept_as(dir, id);
    keep(dir, id, &source, &title)?;
    Ok(kept_as(dir, id) != was)
}

/// What page `id` is made of: its document, and how many bytes the videos and sounds it shows take.
fn kept_as(dir: &Path, id: &str) -> (Option<Vec<u8>>, u64) {
    let media = (std::fs::read_dir(media_folder(dir, id)).into_iter())
        .flatten()
        .flatten()
        .filter_map(|file| Some(file.metadata().ok()?.len()))
        .sum();
    (read_html(dir, id).ok().map(String::into_bytes), media)
}

fn keep(dir: &Path, id: &str, source: &Path, title: &str) -> Result<(), String> {
    let mut page = build(source, Some(&media_folder(dir, id)), None)?;
    if !page.to_ascii_lowercase().contains("<title") {
        page = insert_head(&page, &format!("<title>{}</title>", escape_text(title)));
    }
    let failed = |error: std::io::Error| format!("could not keep the page: {error}");
    write_compressed_html(dir, id, &page).map_err(failed)?;
    let written_in = source.parent().unwrap_or(source);
    std::fs::write(
        written_in_file(dir, id),
        written_in.to_string_lossy().as_bytes(),
    )
    .map_err(failed)?;
    std::fs::write(source_file(dir, id), source.to_string_lossy().as_bytes()).map_err(failed)
}

pub fn source_file(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.source"))
}

/// The folder beside page `id` that holds the videos and sounds it shows.
pub fn media_folder(dir: &Path, id: &str) -> PathBuf {
    dir.join(id)
}

/// The file naming the folder page `id` was written in.
pub fn written_in_file(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.written-in"))
}

/// The folder page `id` was written in, while it is still there.
pub fn written_in(dir: &Path, id: &str) -> Option<PathBuf> {
    let named = std::fs::read_to_string(written_in_file(dir, id)).ok()?;
    Some(PathBuf::from(named)).filter(|folder| folder.is_dir())
}

/// The title of page `id` kept in `dir`, as its `<title>` names it; none when no such page is kept.
pub fn title(dir: &Path, id: &str) -> Option<String> {
    let path = page_file(dir, id)?;
    let page = if is_compressed_page(&path) {
        let file = std::fs::File::open(&path).ok()?;
        let mut decoder = zstd::stream::read::Decoder::new(file).ok()?;
        let mut head = Vec::new();
        std::io::Read::read_to_end(
            &mut std::io::Read::take(&mut decoder, HEAD_BYTES),
            &mut head,
        )
        .ok()?;
        let head_text = String::from_utf8_lossy(&head);
        if head_text.to_ascii_lowercase().contains("</title") {
            head_text.into_owned()
        } else {
            read_raw_html_file(&path).ok()?
        }
    } else {
        let mut head = Vec::new();
        std::io::Read::read_to_end(
            &mut std::io::Read::take(std::fs::File::open(&path).ok()?, HEAD_BYTES),
            &mut head,
        )
        .ok()?;
        let head = String::from_utf8_lossy(&head);
        match head.to_ascii_lowercase().contains("</title") {
            true => head.into_owned(),
            false => std::fs::read_to_string(&path).ok()?,
        }
    };
    let start = page.to_ascii_lowercase().find("<title")?;
    let (_, rest) = page.get(start..)?.split_once('>')?;
    let (title, _) = rest.split_once('<')?;
    let title = (title
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'"))
    .replace("&amp;", "&");
    let title: String = one_line(&title).chars().take(MAX_TITLE_CHARS).collect();
    Some(match title.is_empty() {
        true => "Page".to_owned(),
        false => title,
    })
}

const BASE_CSS: &str = "html{background:var(--background,Canvas);color:var(--foreground,CanvasText);font-family:var(--font-sans,system-ui);font-size:14px;line-height:1.5;-webkit-font-smoothing:antialiased;-webkit-text-size-adjust:100%}html[data-sikemux-framed]{background:transparent;overflow:hidden!important}body{margin:0}code,kbd,pre,samp{font-family:var(--font-mono,ui-monospace)}";

/// The window's theme as the `:root` rule pages style against. Only custom
/// property names pass, and no value can close the rule or the style element.
pub fn theme_rule(dark: bool, variables: &BTreeMap<String, String>) -> String {
    let mut rule = format!(
        ":root{{color-scheme:{};",
        if dark { "dark" } else { "light" }
    );
    for (name, value) in variables {
        let named = (name.strip_prefix("--")).is_some_and(|rest| {
            !rest.is_empty()
                && (rest.bytes())
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        });
        if named {
            let value: String = value.chars().filter(|c| !";{}<>".contains(*c)).collect();
            rule.push_str(&format!("{name}:{value};"));
        }
    }
    rule + "}"
}

/// `html` with nothing on the network open to it but `origins`, which it may fetch from and
/// show pictures from: no frames or form posts, and pictures, fonts and media only as inlined.
pub fn network_closed(html: &str, origins: &[String]) -> String {
    let reached = origins.join(" ");
    let (pictures, connect) = match reached.is_empty() {
        true => (String::new(), "'none'"),
        false => (format!(" {reached}"), reached.as_str()),
    };
    let policy = format!(
        "default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval'; style-src 'unsafe-inline'; img-src data: blob:{pictures}; font-src data: blob:; media-src data: blob:; connect-src {connect}; form-action 'none'; base-uri 'none'"
    );
    insert_head(
        html,
        &format!(
            "<meta http-equiv=\"Content-Security-Policy\" content=\"{policy}\"><meta http-equiv=\"x-dns-prefetch-control\" content=\"off\">"
        ),
    )
}

/// A page as a window shows it: `theme`, a [`theme_rule`], and the base styles
/// ahead of everything the agent wrote, with the window's own `script` first.
pub fn themed(html: &str, theme: &str, script: Option<&str>) -> String {
    let script = script.map_or_else(String::new, |script| format!("<script>{script}</script>"));
    let markup = format!(
        "<meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><style id=\"sikemux-theme\">{theme}</style><style>{BASE_CSS}</style>{script}"
    );
    insert_head(html, &markup)
}

/// Resolves a relative URL reference inside `root`, refusing absolute URLs, traversal and hidden names.
fn safe_under(root: &Path, reference: &str) -> Option<PathBuf> {
    let path = reference
        .trim()
        .split(['?', '#'])
        .next()
        .unwrap_or_default();
    if path.is_empty()
        || path.starts_with('/')
        || path.starts_with("//")
        || path.contains(':')
        || path.contains('\\')
    {
        return None;
    }
    let mut relative = PathBuf::new();
    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        if segment == "." {
            continue;
        }
        if segment == ".." || segment.starts_with('.') || segment.contains('\0') {
            return None;
        }
        relative.push(segment);
    }
    if relative.as_os_str().is_empty() {
        return None;
    }
    let base = sikemux_process::files::real_path(root).ok()?;
    let found = sikemux_process::files::real_path(base.join(relative)).ok()?;
    (found.starts_with(&base) && found.is_file()).then_some(found)
}

fn data_url_for(
    path: &Path,
    label: &str,
    total: &mut usize,
    old_len: usize,
    max_bytes: usize,
    cache: &mut HashMap<PathBuf, String>,
) -> Result<String, String> {
    let url = if let Some(cached) = cache.get(path) {
        let next = total.saturating_sub(old_len).saturating_add(cached.len());
        if next > max_bytes {
            return Err(too_large_media(label, path));
        }
        *total = next;
        cached.clone()
    } else {
        let len = std::fs::metadata(path)
            .map_err(|_| format!("This artifact refers to {label}, which is no longer on disk."))?
            .len();
        let mime = content_type(path);
        let encoded_len = 5usize
            .saturating_add(mime.len())
            .saturating_add(8)
            .saturating_add(((len as usize).saturating_add(2) / 3).saturating_mul(4));
        let next = total.saturating_sub(old_len).saturating_add(encoded_len);
        if next > max_bytes {
            return Err(too_large_media(label, path));
        }
        let bytes = std::fs::read(path)
            .map_err(|error| format!("Could not read {label} to share: {error}"))?;
        use base64::Engine;
        let encoded = format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        );
        let next = total.saturating_sub(old_len).saturating_add(encoded.len());
        if next > max_bytes {
            return Err(too_large_media(label, path));
        }
        *total = next;
        cache.insert(path.to_owned(), encoded.clone());
        encoded
    };
    Ok(url)
}

fn too_large_media(label: &str, path: &Path) -> String {
    let mb = std::fs::metadata(path)
        .ok()
        .map(|meta| meta.len() as f64 / MIB as f64);
    match mb {
        Some(mb) if mb >= 0.1 => format!(
            "This artifact's media ({label}, {mb:.1} MiB) is too large to share; shared artifacts are limited to 25 MiB."
        ),
        _ => "This artifact is too large to share with its media (limit is 25 MiB).".into(),
    }
}

/// Self-contains a kept page for sharing: inlines small media from `{id}/` and scripts, styles or
/// media beside the page within `max_bytes`, and refuses rather than sending a broken copy.
pub fn for_share(dir: &Path, id: &str, html: &str, max_bytes: usize) -> Result<String, String> {
    if html.len() > max_bytes {
        return Err("This artifact is too large to share (limit is 25 MiB).".into());
    }
    let mut total = html.len();
    let mut cache: HashMap<PathBuf, String> = HashMap::new();
    let mut failure: Option<String> = None;

    // 1. Inline video and audio copied into `{dir}/{id}/` when the page was published.
    let prefix = format!("{id}/");
    let media_dir = media_folder(dir, id);
    let page = KEPT_MEDIA
        .replace_all(html, |found: &regex::Captures| {
            if failure.is_some() {
                return found[0].to_owned();
            }
            let (open, reference, close) = (&found[1], &found[2], &found[3]);
            let paired = (open == "(" && close == ")") || (open != "(" && open == close);
            let Some(rel) = reference.strip_prefix(&prefix).filter(|_| paired) else {
                return found[0].to_owned();
            };
            let Some(path) = safe_under(&media_dir, rel) else {
                failure = Some(format!(
                    "This artifact refers to media ({rel}) that is no longer on disk."
                ));
                return found[0].to_owned();
            };
            match data_url_for(
                &path,
                rel,
                &mut total,
                reference.len(),
                max_bytes,
                &mut cache,
            ) {
                Ok(data) => format!("{open}{data}{close}"),
                Err(error) => {
                    failure = Some(error);
                    found[0].to_owned()
                }
            }
        })
        .into_owned();
    if let Some(error) = failure {
        return Err(error);
    }

    let Some(folder) = written_in(dir, id) else {
        return Ok(page);
    };

    // 2. Inline local scripts and stylesheets beside the page.
    let page = SCRIPT_SRC
        .replace_all(&page, |found: &regex::Captures| {
            if failure.is_some() {
                return found[0].to_owned();
            }
            let src = found
                .get(2)
                .or_else(|| found.get(3))
                .map_or("", |m| m.as_str());
            let Some(path) = safe_under(&folder, src) else {
                return found[0].to_owned();
            };
            let len = match std::fs::metadata(&path) {
                Ok(meta) => meta.len() as usize,
                Err(_) => return found[0].to_owned(),
            };
            if total.saturating_sub(found[0].len()).saturating_add(len) > max_bytes {
                failure = Some(too_large_media(src, &path));
                return found[0].to_owned();
            }
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => CLOSING_SCRIPT.replace_all(&text, r"<\/script").into_owned(),
                Err(error) => {
                    failure = Some(format!("Could not read {src} to share: {error}"));
                    return found[0].to_owned();
                }
            };
            let attrs = format!("{}{}", found[1].trim_end(), &found[4]);
            let replaced = format!("<script{attrs}>{text}</script>");
            let next = total
                .saturating_sub(found[0].len())
                .saturating_add(replaced.len());
            if next > max_bytes {
                failure = Some(too_large_media(src, &path));
                return found[0].to_owned();
            }
            total = next;
            replaced
        })
        .into_owned();
    if let Some(error) = failure {
        return Err(error);
    }

    let page = LINK_TAG
        .replace_all(&page, |found: &regex::Captures| {
            if failure.is_some() {
                return found[0].to_owned();
            }
            let attrs = &found[1];
            let mut is_stylesheet = false;
            let mut href = None;
            for attr in ATTR.captures_iter(attrs) {
                let name = attr[1].to_ascii_lowercase();
                let value = attr
                    .get(2)
                    .or_else(|| attr.get(3))
                    .map_or("", |m| m.as_str());
                if name == "rel"
                    && value
                        .split_whitespace()
                        .any(|part| part.eq_ignore_ascii_case("stylesheet"))
                {
                    is_stylesheet = true;
                } else if name == "href" {
                    href = Some(value.to_owned());
                }
            }
            let Some(href) = href.filter(|_| is_stylesheet) else {
                return found[0].to_owned();
            };
            let Some(path) = safe_under(&folder, &href) else {
                return found[0].to_owned();
            };
            let len = match std::fs::metadata(&path) {
                Ok(meta) => meta.len() as usize,
                Err(_) => return found[0].to_owned(),
            };
            if total.saturating_sub(found[0].len()).saturating_add(len) > max_bytes {
                failure = Some(too_large_media(&href, &path));
                return found[0].to_owned();
            }
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => CLOSING_STYLE.replace_all(&text, r"<\/style").into_owned(),
                Err(error) => {
                    failure = Some(format!("Could not read {href} to share: {error}"));
                    return found[0].to_owned();
                }
            };
            let replaced = format!("<style>{text}</style>");
            let next = total
                .saturating_sub(found[0].len())
                .saturating_add(replaced.len());
            if next > max_bytes {
                failure = Some(too_large_media(&href, &path));
                return found[0].to_owned();
            }
            total = next;
            replaced
        })
        .into_owned();
    if let Some(error) = failure {
        return Err(error);
    }

    // 3. Inline remaining relative media/tracks/images beside the page; refuse embedded local files
    // that a sandboxed shared link cannot load.
    let page = RESOURCE_TAG
        .replace_all(&page, |found: &regex::Captures| {
            if failure.is_some() {
                return found[0].to_owned();
            }
            let tag = found[1].to_ascii_lowercase();
            let attrs = &found[2];
            let unshareable = matches!(tag.as_str(), "iframe" | "object" | "embed");
            let rewritten = ATTR.replace_all(attrs, |attr: &regex::Captures| {
                if failure.is_some() {
                    return attr[0].to_owned();
                }
                let name = attr[1].to_ascii_lowercase();
                let wants = match unshareable {
                    true => matches!(name.as_str(), "src" | "data"),
                    false => matches!(name.as_str(), "src" | "poster"),
                };
                if !wants {
                    return attr[0].to_owned();
                }
                let value = attr
                    .get(2)
                    .or_else(|| attr.get(3))
                    .map_or("", |m| m.as_str());
                let Some(path) = safe_under(&folder, value) else {
                    return attr[0].to_owned();
                };
                if unshareable {
                    failure = Some(format!(
                        "This artifact loads local file {value}, which cannot be shared through a link."
                    ));
                    return attr[0].to_owned();
                }
                match data_url_for(&path, value, &mut total, value.len(), max_bytes, &mut cache) {
                    Ok(data) => format!("{}=\"{data}\"", &attr[1]),
                    Err(error) => {
                        failure = Some(error);
                        attr[0].to_owned()
                    }
                }
            });
            format!("<{}{rewritten}>", &found[1])
        })
        .into_owned();
    if let Some(error) = failure {
        return Err(error);
    }
    if page.len() > max_bytes {
        return Err("This artifact is too large to share (limit is 25 MiB).".into());
    }
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_goes_after_the_doctype_or_first() {
        assert_eq!(
            insert_head("<!DOCTYPE html>\n<html><p>hi", "<x>"),
            "<!DOCTYPE html><x>\n<html><p>hi"
        );
        assert_eq!(
            insert_head("  <!doctype html><p>", "<x>"),
            "  <!doctype html><x><p>"
        );
        assert_eq!(insert_head("<html><p>", "<x>"), "<x><html><p>");
        assert_eq!(insert_head("\u{feff}<p>", "<x>"), "<x><p>");
    }

    #[test]
    #[cfg(unix)]
    fn local_pictures_are_inlined_and_remote_ones_left() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("shots")).unwrap();
        std::fs::write(dir.path().join("shots/a.png"), [1u8, 2, 3]).unwrap();
        let absolute = dir.path().join("shots/a.png");
        let source = dir.path().join("page.html");
        std::fs::write(
            &source,
            format!(
                "<img src=\"shots/a.png\"><div style=\"background:url({})\"></div><script>const s='{}'</script><img src=\"https://x.test/b.png\"><img src=\"//cdn.test/c.png\">",
                absolute.display(),
                absolute.display()
            ),
        )
        .unwrap();
        let page = build(&source, None, None).unwrap();
        assert_eq!(page.matches("data:image/png;base64,AQID").count(), 3);
        assert!(page.contains("https://x.test/b.png"));
        assert!(page.contains("//cdn.test/c.png"));
    }

    #[test]
    fn local_fonts_are_inlined() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Figtree.woff2"), [7u8]).unwrap();
        let source = dir.path().join("page.html");
        std::fs::write(
            &source,
            "<style>@font-face{font-family:F;src:url(Figtree.woff2) format(\"woff2\")}</style>",
        )
        .unwrap();
        let page = build(&source, None, None).unwrap();
        assert!(
            page.contains("url(data:font/woff2;base64,Bw==) format(\"woff2\")"),
            "{page}"
        );
    }

    #[test]
    fn a_kept_page_keeps_its_video_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ademark.mp4"), [9u8; 4]).unwrap();
        let source = dir.path().join("draft.html");
        std::fs::write(&source, "<video src=\"ademark.mp4\" controls></video>").unwrap();
        let kept = dir.path().join("kept");
        let page = publish(&kept, &source, "Draft").unwrap();
        std::fs::remove_file(dir.path().join("ademark.mp4")).unwrap();
        assert!(compressed_file(&kept, &page.id).is_file());
        assert!(!plain_file(&kept, &page.id).exists());
        let html = read_html(&kept, &page.id).unwrap();
        let video = format!("{}/0-ademark.mp4", page.id);
        assert!(html.contains(&format!("src=\"{video}\"")), "{html}");
        assert_eq!(std::fs::read(kept.join(video)).unwrap(), [9u8; 4]);
    }

    #[test]
    fn a_page_shown_again_by_id_has_the_file_as_it_is_now() {
        let dir = tempfile::tempdir().unwrap();
        let kept = dir.path().join("kept");
        let source = dir.path().join("mockup.html");
        std::fs::write(&source, "<p>first</p>").unwrap();
        let page = publish(&kept, &source, "Mockup").unwrap();
        std::fs::write(&source, "<p>redrawn</p>").unwrap();
        assert!(refresh(&kept, &page.id).unwrap());
        let html = read_html(&kept, &page.id).unwrap();
        assert!(html.contains("redrawn") && html.contains("<title>Mockup</title>"));
    }

    #[test]
    fn a_missing_picture_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("page.html");
        std::fs::write(&source, "<img src=\"gone.webp\">").unwrap();
        let error = build(&source, None, None).unwrap_err();
        assert!(error.contains("gone.webp"), "{error}");
    }

    #[test]
    fn only_page_ids_are_served() {
        assert!(is_page_id("0123456789abcdef0123456789abcdef"));
        assert!(!is_page_id("../../etc/passwd"));
        assert!(!is_page_id("0123456789abcdef0123456789abcde"));
        assert_eq!(
            page_id_of(Path::new("/tmp/0123456789abcdef0123456789abcdef.html.zst")),
            Some("0123456789abcdef0123456789abcdef")
        );
        assert_eq!(
            page_id_of(Path::new("/tmp/0123456789abcdef0123456789abcdef.html")),
            Some("0123456789abcdef0123456789abcdef")
        );
    }

    #[test]
    fn pages_compress_deduplicate_blobs_and_gc_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let kept = dir.path().join("kept");
        let shared_font: Vec<u8> = (0..8192u32).map(|i| ((i * 31) % 251) as u8).collect();
        std::fs::write(dir.path().join("Figtree.woff2"), &shared_font).unwrap();
        let unique_img: Vec<u8> = (0..4096u32).map(|i| ((i * 17 + 5) % 251) as u8).collect();
        std::fs::write(dir.path().join("shot.png"), &unique_img).unwrap();

        let source_a = dir.path().join("a.html");
        let repeated_markup = "<p>Some repeated report body text.</p>".repeat(200);
        std::fs::write(
            &source_a,
            format!(
                "<!doctype html><style>@font-face{{src:url(Figtree.woff2)}}</style><img src=\"shot.png\">{repeated_markup}"
            ),
        )
        .unwrap();
        let source_b = dir.path().join("b.html");
        std::fs::write(
            &source_b,
            format!(
                "<!doctype html><style>@font-face{{src:url(Figtree.woff2)}}</style>{repeated_markup}"
            ),
        )
        .unwrap();

        let page_a = publish(&kept, &source_a, "Report A").unwrap();
        let page_b = publish(&kept, &source_b, "Report B").unwrap();

        let expected_a = insert_head(
            &build(&source_a, None, None).unwrap(),
            "<title>Report A</title>",
        );
        let expected_b = insert_head(
            &build(&source_b, None, None).unwrap(),
            "<title>Report B</title>",
        );
        assert_eq!(read_html(&kept, &page_a.id).unwrap(), expected_a);
        assert_eq!(read_html(&kept, &page_b.id).unwrap(), expected_b);
        assert_eq!(title(&kept, &page_a.id).as_deref(), Some("Report A"));
        assert_eq!(title(&kept, &page_b.id).as_deref(), Some("Report B"));

        // Shared font is stored once in blobs/ alongside the unique image (2 blobs total).
        let blobs_count = std::fs::read_dir(blobs_dir(&kept)).unwrap().count();
        assert_eq!(blobs_count, 2);

        // Compressed files + deduplicated blobs must be much smaller than raw duplicated HTML.
        let compressed_bytes = std::fs::metadata(compressed_file(&kept, &page_a.id))
            .unwrap()
            .len()
            + std::fs::metadata(compressed_file(&kept, &page_b.id))
                .unwrap()
                .len();
        let blob_bytes: u64 = std::fs::read_dir(blobs_dir(&kept))
            .unwrap()
            .flatten()
            .map(|e| e.metadata().unwrap().len())
            .sum();
        assert!(
            (compressed_bytes + blob_bytes) < (expected_a.len() + expected_b.len()) as u64 / 2,
            "expected >50% reduction, got {} vs {}",
            compressed_bytes + blob_bytes,
            expected_a.len() + expected_b.len()
        );

        // Removing page A and running GC drops only the unreferenced image blob, keeping the shared font.
        remove_page_files(&kept, &page_a.id);
        assert_eq!(gc_blobs_older_than(&kept, Duration::ZERO), 1);
        assert_eq!(std::fs::read_dir(blobs_dir(&kept)).unwrap().count(), 1);
        assert_eq!(read_html(&kept, &page_b.id).unwrap(), expected_b);

        // Legacy uncompressed `.html` files read cleanly and compact in place.
        let legacy_id = "0123456789abcdef0123456789abcdef";
        std::fs::write(plain_file(&kept, legacy_id), &expected_a).unwrap();
        assert_eq!(read_html(&kept, legacy_id).unwrap(), expected_a);
        assert_eq!(title(&kept, legacy_id).as_deref(), Some("Report A"));
        assert_eq!(compact_dir_older_than(&kept, Duration::ZERO), 1);
        assert!(compressed_file(&kept, legacy_id).is_file());
        assert!(!plain_file(&kept, legacy_id).exists());
        assert_eq!(read_html(&kept, legacy_id).unwrap(), expected_a);
    }

    #[test]
    fn sharing_inlines_small_media_and_refuses_when_over_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("clip.mp4"), [1u8, 2, 3, 4]).unwrap();
        std::fs::write(dir.path().join("app.js"), "console.log('hi');").unwrap();
        std::fs::write(dir.path().join("look.css"), "body{color:red}").unwrap();
        let source = dir.path().join("demo.html");
        std::fs::write(
            &source,
            "<link rel=\"stylesheet\" href=\"look.css\"><video src=\"clip.mp4\"></video><script src=\"app.js\"></script>",
        )
        .unwrap();
        let kept = dir.path().join("kept");
        let page = publish(&kept, &source, "Demo").unwrap();
        let html = read_html(&kept, &page.id).unwrap();
        assert!(html.contains(&format!("{}/0-clip.mp4", page.id)));

        let shared = for_share(&kept, &page.id, &html, MAX_SHARED_HTML_BYTES).unwrap();
        assert!(
            shared.contains("data:video/mp4;base64,AQIDBA=="),
            "{shared}"
        );
        assert!(
            shared.contains("<style>body{color:red}</style>"),
            "{shared}"
        );
        assert!(
            shared.contains("<script>console.log('hi');</script>"),
            "{shared}"
        );
        assert!(!shared.contains(&format!("{}/0-clip.mp4", page.id)));

        let too_big_clip = dir.path().join("big.mp4");
        std::fs::write(&too_big_clip, [7u8; 256]).unwrap();
        std::fs::write(&source, "<video src=\"big.mp4\"></video>").unwrap();
        let big_page = publish(&kept, &source, "Big").unwrap();
        let big_html = read_html(&kept, &big_page.id).unwrap();
        let too_big = for_share(&kept, &big_page.id, &big_html, big_html.len() + 16).unwrap_err();
        assert!(too_big.contains("too large"), "{too_big}");

        std::fs::write(dir.path().join("child.html"), "<p>nested</p>").unwrap();
        std::fs::write(&source, "<iframe src=\"child.html\"></iframe>").unwrap();
        let nested = publish(&kept, &source, "Nested").unwrap();
        let nested_html = read_html(&kept, &nested.id).unwrap();
        let refused =
            for_share(&kept, &nested.id, &nested_html, MAX_SHARED_HTML_BYTES).unwrap_err();
        assert!(refused.contains("child.html"), "{refused}");
    }
}
