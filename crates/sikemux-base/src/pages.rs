//! Pages agents show the person: a self-contained HTML document an agent
//! writes, kept in the data folder with every local picture and font it names
//! inlined, so it outlives those files and any client can show it.

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
const MAX_PAGE_BYTES: usize = 25 * MIB as usize;
const MAX_TITLE_CHARS: usize = 120;
const MAX_HEIGHT: u32 = 4000;

static LOCAL_FILE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(["'(])\s*([^"'()<>\s:]+?\.(?i:png|jpe?g|gif|webp|avif|svg|bmp|ico|woff2?|ttf|otf))\s*(["')])"#,
    )
    .expect("local file pattern")
});

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PageRef {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
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
pub fn build(source: &Path) -> Result<String, String> {
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
        match image_data(&resolve(folder, reference)) {
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
            "with its pictures and fonts inlined the page is {:.1} MiB; the limit is 25 MiB",
            page.len() as f64 / MIB as f64
        ));
    }
    Ok(page.into_owned())
}

fn resolve(folder: &Path, reference: &str) -> PathBuf {
    folder.join(sikemux_process::paths::expand_home(reference))
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
        _ => "application/octet-stream",
    }
}

/// Keeps the page an agent wrote at `source` in `dir` and names it for the
/// chat.
pub fn publish(
    dir: &Path,
    source: &Path,
    title: &str,
    height: Option<u32>,
) -> Result<PageRef, String> {
    let title: String = title.trim().chars().take(MAX_TITLE_CHARS).collect();
    if title.is_empty() {
        return Err("title must be nonempty".into());
    }
    if height.is_some_and(|height| height == 0 || height > MAX_HEIGHT) {
        return Err(format!("height must be between 1 and {MAX_HEIGHT}"));
    }
    let mut page = build(source)?;
    if !page.to_ascii_lowercase().contains("<title") {
        page = insert_head(&page, &format!("<title>{}</title>", escape_text(&title)));
    }
    std::fs::create_dir_all(dir).map_err(|error| format!("could not keep the page: {error}"))?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    std::fs::write(dir.join(format!("{id}.html")), page)
        .map_err(|error| format!("could not keep the page: {error}"))?;
    Ok(PageRef { id, title, height })
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
        let page = build(&source).unwrap();
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
        let page = build(&source).unwrap();
        assert!(
            page.contains("url(data:font/woff2;base64,Bw==) format(\"woff2\")"),
            "{page}"
        );
    }

    #[test]
    fn a_missing_picture_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("page.html");
        std::fs::write(&source, "<img src=\"gone.webp\">").unwrap();
        let error = build(&source).unwrap_err();
        assert!(error.contains("gone.webp"), "{error}");
    }

    #[test]
    fn only_page_ids_are_served() {
        assert!(is_page_id("0123456789abcdef0123456789abcdef"));
        assert!(!is_page_id("../../etc/passwd"));
        assert!(!is_page_id("0123456789abcdef0123456789abcde"));
    }
}
