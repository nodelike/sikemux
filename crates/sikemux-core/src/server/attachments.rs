//! Files devices send for a chat. They are kept where the app keeps pictures
//! pasted into its chats, outside every project, and the agent is given their
//! path like any other attachment.

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::STANDARD, Engine};

use crate::protocol::MAX_ATTACHMENT_BYTES;

use super::{CoreError, CoreResult};

const MAX_STEM_CHARS: usize = 100;
const MAX_EXTENSION_CHARS: usize = 10;

pub(crate) const TOO_LARGE: &str = "a file sent to a chat can be at most 10 MB";

/// The app's `pasted` folder: its cache folder carries the same name as the
/// data folder it hands the core.
pub(crate) fn pasted_dir(data_dir: &Path) -> Option<PathBuf> {
    Some(cache_root()?.join(data_dir.file_name()?).join("pasted"))
}

fn cache_root() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return Some(home?.join("Library/Caches"));
    }
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| Some(home?.join(".cache")))
}

/// Writes the file into `dir` under a cleaned-up `name`, never over another
/// file, and answers with its path.
pub(crate) fn save(dir: &Path, name: &str, mime: &str, data: &str) -> CoreResult<PathBuf> {
    if data.len() / 4 * 3 > MAX_ATTACHMENT_BYTES + 2 {
        return Err(TOO_LARGE.into());
    }
    let bytes = STANDARD
        .decode(data)
        .map_err(|_| CoreError::from("the file's contents are not base64"))?;
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err(TOO_LARGE.into());
    }
    save_bytes(dir, name, mime, &bytes)
}

/// Writes `bytes` into `dir` under a cleaned-up `name`, never over another
/// file, and answers with its path.
pub(crate) fn save_bytes(dir: &Path, name: &str, mime: &str, bytes: &[u8]) -> CoreResult<PathBuf> {
    fs::create_dir_all(dir)
        .map_err(|error| CoreError::from(format!("could not keep the file: {error}")))?;
    let (stem, extension) = file_name(name, mime);
    let (path, mut file) = create_new(dir, &stem, &extension)?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&path);
        return Err(format!("could not keep the file: {error}").into());
    }
    Ok(path)
}

/// Whether `path` is in `dir`, however it is spelled or linked.
pub(crate) fn is_kept(dir: &Path, path: &Path) -> bool {
    match (fs::canonicalize(dir), fs::canonicalize(path)) {
        (Ok(dir), Ok(path)) => path.starts_with(dir),
        _ => false,
    }
}

/// A picture scaled to fit `max_edge` pixels on its longest side, as a JPEG in base64, for a
/// device to show small; none where this host cannot scale it, which then sends the picture.
pub(crate) fn thumbnail(path: &Path, max_edge: u32) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let picture = [
        "png", "jpg", "jpeg", "heic", "heif", "gif", "webp", "tif", "tiff", "bmp",
    ];
    if !picture.contains(&extension.as_str()) {
        return None;
    }
    let out = std::env::temp_dir().join(format!(
        "sikemux-thumbnail-{}.jpg",
        uuid::Uuid::new_v4().simple()
    ));
    let scaled = sikemux_process::user_environment::command("/usr/bin/sips")
        .args(["-s", "format", "jpeg", "-s", "formatOptions", "80", "-Z"])
        .arg(max_edge.clamp(16, 2048).to_string())
        .arg(path)
        .arg("--out")
        .arg(&out)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    let bytes = scaled.then(|| fs::read(&out).ok()).flatten();
    let _ = fs::remove_file(&out);
    bytes.map(|bytes| STANDARD.encode(bytes))
}

/// A file, in base64, for a device to show.
pub(crate) fn read(path: &Path) -> CoreResult<String> {
    let missing = || CoreError::from("this host no longer has that file");
    let metadata = fs::metadata(path).map_err(|_| missing())?;
    if !metadata.is_file() {
        return Err(missing());
    }
    if metadata.len() > MAX_ATTACHMENT_BYTES as u64 {
        return Err("a device can be sent files of at most 10 MB".into());
    }
    let bytes = fs::read(path).map_err(|_| missing())?;
    Ok(STANDARD.encode(bytes))
}

fn create_new(dir: &Path, stem: &str, extension: &str) -> CoreResult<(PathBuf, File)> {
    for n in 0..1000 {
        let candidate = if n == 0 {
            format!("{stem}{extension}")
        } else {
            format!("{stem} ({n}){extension}")
        };
        let path = dir.join(candidate);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("could not keep the file: {error}").into()),
        }
    }
    Err("too many files with that name".into())
}

/// The stem and the extension, dot included, of the name a device gave. Only
/// its last part counts, and only characters safe in any folder are kept.
fn file_name(name: &str, mime: &str) -> (String, String) {
    let leaf = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = leaf
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " ._-()+,@".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.');
    let (stem, extension) = match cleaned.rsplit_once('.') {
        Some((stem, extension))
            if !extension.is_empty()
                && extension.len() <= MAX_EXTENSION_CHARS
                && extension.chars().all(|c| c.is_ascii_alphanumeric()) =>
        {
            (stem, Some(extension.to_owned()))
        }
        _ => (cleaned, None),
    };
    let stem: String = stem.trim().chars().take(MAX_STEM_CHARS).collect();
    let stem = if stem.is_empty() {
        "attachment".to_owned()
    } else {
        stem
    };
    let extension = extension.or_else(|| extension_for(mime).map(str::to_owned));
    (
        stem,
        extension.map(|ext| format!(".{ext}")).unwrap_or_default(),
    )
}

fn extension_for(mime: &str) -> Option<&'static str> {
    Some(match mime.trim().to_ascii_lowercase().as_str() {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/heic" => "heic",
        "application/pdf" => "pdf",
        "text/plain" => "txt",
        "text/markdown" => "md",
        "application/json" => "json",
        "application/zip" => "zip",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str, mime: &str) -> String {
        let (stem, extension) = file_name(name, mime);
        format!("{stem}{extension}")
    }

    #[test]
    fn a_name_keeps_only_its_last_part_and_safe_characters() {
        assert_eq!(named("../../etc/passwd", ""), "passwd");
        assert_eq!(named("..\\..\\boot.ini", ""), "boot.ini");
        assert_eq!(
            named("Screen Shot 2026-10-06.png", ""),
            "Screen Shot 2026-10-06.png"
        );
        assert_eq!(named("rapport été.pdf", ""), "rapport été.pdf");
        assert_eq!(named("a:b*c?.txt", ""), "a_b_c_.txt");
        assert_eq!(named("line\nbreak.txt", ""), "line_break.txt");
    }

    #[test]
    fn a_name_never_hides_the_file_or_climbs_out() {
        assert_eq!(named("..", ""), "attachment");
        assert_eq!(named(".env", ""), "env");
        assert_eq!(named("...hidden.txt", ""), "hidden.txt");
        assert_eq!(named("", "image/png"), "attachment.png");
        assert_eq!(named("/", ""), "attachment");
    }

    #[test]
    fn a_name_without_an_extension_takes_one_from_its_kind() {
        assert_eq!(named("IMG_0042", "image/jpeg"), "IMG_0042.jpg");
        assert_eq!(named("notes", "application/x-unknown"), "notes");
        assert_eq!(named("photo.HEIC", "image/jpeg"), "photo.HEIC");
        assert_eq!(named("archive.tar.gz", ""), "archive.tar.gz");
    }

    #[test]
    fn a_long_name_is_cut_short_and_keeps_its_extension() {
        let (stem, extension) = file_name(&format!("{}.png", "x".repeat(400)), "");
        assert_eq!(stem.chars().count(), MAX_STEM_CHARS);
        assert_eq!(extension, ".png");
    }

    #[test]
    fn a_saved_file_never_replaces_another() {
        let dir = tempfile::tempdir().expect("dir");
        let pasted = dir.path().join("pasted");
        let data = STANDARD.encode(b"first");
        let first = save(&pasted, "shot.png", "image/png", &data).expect("first");
        let second = save(
            &pasted,
            "shot.png",
            "image/png",
            &STANDARD.encode(b"second"),
        )
        .expect("second");
        assert_eq!(first, pasted.join("shot.png"));
        assert_eq!(second, pasted.join("shot (1).png"));
        assert_eq!(fs::read(&first).expect("read"), b"first");
        assert_eq!(fs::read(&second).expect("read"), b"second");
    }

    #[test]
    fn a_file_over_the_limit_or_not_in_base64_is_refused() {
        let dir = tempfile::tempdir().expect("dir");
        let big = STANDARD.encode(vec![0u8; MAX_ATTACHMENT_BYTES + 1]);
        assert_eq!(
            save(dir.path(), "big.bin", "", &big)
                .unwrap_err()
                .to_string(),
            TOO_LARGE
        );
        let exact = STANDARD.encode(vec![0u8; MAX_ATTACHMENT_BYTES]);
        assert!(save(dir.path(), "exact.bin", "", &exact).is_ok());
        assert!(save(dir.path(), "bad.txt", "", "not base64!")
            .unwrap_err()
            .to_string()
            .contains("base64"));
        assert!(!dir.path().join("big.bin").exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_app_s_pasted_folder_is_in_its_cache_folder() {
        let dir = pasted_dir(Path::new(
            "/Users/me/Library/Application Support/com.nodelike.sikemux",
        ))
        .expect("dir");
        assert!(dir.ends_with("Library/Caches/com.nodelike.sikemux/pasted"));
    }
}
