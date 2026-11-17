//! Pages agents show the person: an HTML document an agent writes, kept in the
//! data folder with every local picture and font it names inlined and every video
//! or sound copied into a folder beside it, so it outlives those files. Whatever
//! else it loads comes from the folder it was written in.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

/// The folder under the data folder pages are kept in.
pub const FOLDER: &str = "pages";

const MIB: u64 = 1024 * 1024;
const MAX_SOURCE_BYTES: u64 = 2 * MIB;
const MAX_IMAGE_BYTES: u64 = 10 * MIB;
/// Under the 16 MiB a connection to the core carries, so a reply can always fetch the page.
const MAX_PAGE_BYTES: usize = 15 * MIB as usize;
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
    Regex::new(&format!(
        r#"(["'(])\s*({DRIVE}[^"'()<>\s:]+?\.(?i:png|jpe?g|gif|webp|avif|svg|bmp|ico|woff2?|ttf|otf|mp4|m4v|webm|mov|mp3|m4a|wav|ogg|oga|opus|flac|aac))\s*(["')])"#,
    ))
    .expect("local file pattern")
});

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PageRef {
    pub id: String,
    pub title: String,
}

pub fn is_page_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/* Elements ahead of `<html>` land in the head the parser makes for them, so
the markup goes straight after the doctype and needs no parsing of the page. */
pub fn insert_head(html: &str, markup: &str) -> String {
    let body = html.trim_start_matches('\u{feff}');
    let lead = body.len() - body.trim_start().len();
    let doctype = body[lead..]
        .get(..9)
        .is_some_and(|start| start.eq_ignore_ascii_case("<!doctype"));
    let at = if doctype {
        body[lead..].find('>').map_or(0, |end| lead + end + 1)
    } else {
        0
    };
    format!("{}{markup}{}", &body[..at], &body[at..])
}

fn escape_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Reads the page an agent wrote and inlines every local picture and font it
/// names, by absolute, `~/` or relative path, so the page outlives those files.
/// Videos and sounds are too big to inline: with `media` they are copied into
/// that folder and named relative to its parent, and without it left as named.
pub fn build(source: &Path, media: Option<&Path>) -> Result<String, String> {
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
    let title: String = title.trim().chars().take(MAX_TITLE_CHARS).collect();
    if title.is_empty() {
        return Err("title must be nonempty".into());
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    let media = media_folder(dir, &id);
    let kept = build(source, Some(&media)).and_then(|mut page| {
        if !page.to_ascii_lowercase().contains("<title") {
            page = insert_head(&page, &format!("<title>{}</title>", escape_text(&title)));
        }
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("could not keep the page: {error}"))?;
        std::fs::write(dir.join(format!("{id}.html")), page)
            .map_err(|error| format!("could not keep the page: {error}"))?;
        let written_in = source.parent().unwrap_or(source);
        std::fs::write(
            written_in_file(dir, &id),
            written_in.to_string_lossy().as_bytes(),
        )
        .map_err(|error| format!("could not keep the page: {error}"))
    });
    if let Err(error) = kept {
        let _ = std::fs::remove_dir_all(&media);
        return Err(error);
    }
    Ok(PageRef { id, title })
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
    if !is_page_id(id) {
        return None;
    }
    let path = dir.join(format!("{id}.html"));
    let mut head = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(std::fs::File::open(&path).ok()?, HEAD_BYTES),
        &mut head,
    )
    .ok()?;
    let head = String::from_utf8_lossy(&head);
    let page = match head.to_ascii_lowercase().contains("</title") {
        true => head.into_owned(),
        false => std::fs::read_to_string(&path).ok()?,
    };
    let start = page.to_ascii_lowercase().find("<title")?;
    let (_, rest) = page[start..].split_once('>')?;
    let (title, _) = rest.split_once('<')?;
    let title = (title
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'"))
    .replace("&amp;", "&");
    let title: String = title.trim().chars().take(MAX_TITLE_CHARS).collect();
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
        let named = name.len() > 2
            && name.starts_with("--")
            && name[2..]
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
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
        let page = build(&source, None).unwrap();
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
        let page = build(&source, None).unwrap();
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
        let html = std::fs::read_to_string(kept.join(format!("{}.html", page.id))).unwrap();
        let video = format!("{}/0-ademark.mp4", page.id);
        assert!(html.contains(&format!("src=\"{video}\"")), "{html}");
        assert_eq!(std::fs::read(kept.join(video)).unwrap(), [9u8; 4]);
    }

    #[test]
    fn a_missing_picture_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("page.html");
        std::fs::write(&source, "<img src=\"gone.webp\">").unwrap();
        let error = build(&source, None).unwrap_err();
        assert!(error.contains("gone.webp"), "{error}");
    }

    #[test]
    fn only_page_ids_are_served() {
        assert!(is_page_id("0123456789abcdef0123456789abcdef"));
        assert!(!is_page_id("../../etc/passwd"));
        assert!(!is_page_id("0123456789abcdef0123456789abcde"));
    }
}
