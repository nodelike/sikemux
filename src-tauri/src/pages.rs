//! Pages agents show the person: a self-contained HTML document an agent
//! writes, kept in the app's data folder with every local picture and font it
//! names inlined. The chat shows one over `page://` in a sandboxed frame, so its
//! scripts never reach the app; a desk tab shows a copy over loopback.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use regex::Regex;
use serde::Serialize;
use tauri::http::{header, Request, Response, StatusCode};
use tauri::{AppHandle, Manager, Runtime, State, UriSchemeContext, UriSchemeResponder};

use crate::browser::BrowserManager;
use crate::error::{AppError, AppResult};
use crate::file_serving::content_type;

pub const SCHEME: &str = "page";

const MIB: u64 = 1024 * 1024;
const MAX_SOURCE_BYTES: u64 = 2 * MIB;
const MAX_IMAGE_BYTES: u64 = 10 * MIB;
const MAX_PAGE_BYTES: usize = 25 * MIB as usize;
const MAX_TITLE_CHARS: usize = 120;
const MAX_HEIGHT: u32 = 4000;

/* What a page may do in its frame: run its own scripts and forms, nothing that
reaches the app, its storage or another window. */
const SANDBOX: &str = "sandbox allow-scripts allow-forms";

const BASE_CSS: &str = "html{background:var(--background,Canvas);color:var(--foreground,CanvasText);font-family:var(--font-sans,system-ui);font-size:14px;line-height:1.5;-webkit-font-smoothing:antialiased;-webkit-text-size-adjust:100%}html[data-sikemux-framed]{background:transparent;scrollbar-width:none}html[data-sikemux-framed]::-webkit-scrollbar{display:none}body{margin:0}code,kbd,pre,samp{font-family:var(--font-mono,ui-monospace)}";

/* Speaks the MCP Apps messages (JSON-RPC over postMessage) between a framed
page and the chat: the chat sends theme changes, the page reports its height
and hands over the links the person clicks. A wheel turned over the page that
nothing in it can scroll goes to the chat too, which never sees it otherwise. */
const BOOTSTRAP: &str = r#"(()=>{const root=document.documentElement;if(window.parent===window)return;root.setAttribute("data-sikemux-framed","");const theme=document.getElementById("sikemux-theme");const post=(message)=>window.parent.postMessage({jsonrpc:"2.0",...message},"*");let links=0;window.addEventListener("message",(event)=>{const data=event.data;if(event.source!==window.parent||!data||data.jsonrpc!=="2.0"||data.method!=="ui/notifications/host-context-changed")return;const variables=data.params&&data.params.styles&&data.params.styles.variables;if(!variables||typeof variables!=="object")return;let css=":root{color-scheme:"+(data.params.theme==="light"?"light":"dark")+";";for(const name in variables)if(/^--[a-z0-9-]+$/.test(name))css+=name+":"+String(variables[name]).replace(/[;{}<>]/g,"")+";";theme.textContent=css+"}";});document.addEventListener("click",(event)=>{const link=event.isTrusted&&event.composedPath().find((target)=>target instanceof Element&&target.matches("a[href]"));if(!link)return;let url;try{url=new URL(link.getAttribute("href"),document.baseURI);}catch{return;}if(!/^https?:$/.test(url.protocol))return;event.preventDefault();post({id:"sikemux-link-"+(++links),method:"ui/open-link",params:{url:url.href}});},true);let reported=0;const report=()=>{const height=Math.ceil(root.getBoundingClientRect().height);if(height===reported)return;reported=height;post({method:"ui/notifications/size-changed",params:{height}});};const observer=new ResizeObserver(report);observer.observe(root);document.addEventListener("DOMContentLoaded",()=>{if(document.body)observer.observe(document.body);report();});window.addEventListener("load",report);const scrollsItself=(node,down)=>{for(;node instanceof Element;node=node.parentElement){const style=getComputedStyle(node).overflowY;const scroller=node===root||((style==="auto"||style==="scroll")&&node!==document.body);if(scroller&&(down?node.scrollTop+node.clientHeight<node.scrollHeight-1:node.scrollTop>0))return true;}return false;};window.addEventListener("wheel",(event)=>{if(event.ctrlKey||Math.abs(event.deltaY)<=Math.abs(event.deltaX)||scrollsItself(event.target,event.deltaY>0))return;event.preventDefault();const scale=event.deltaMode===1?16:event.deltaMode===2?window.innerHeight:1;post({method:"sikemux/wheel",params:{deltaY:event.deltaY*scale}});},{passive:false});})();"#;

static LOCAL_FILE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(["'(])\s*([^"'()<>\s:]+?\.(?i:png|jpe?g|gif|webp|avif|svg|bmp|ico|woff2?|ttf|otf))\s*(["')])"#,
    )
    .expect("local file pattern")
});

/// The theme the window last handed over, as the `:root` rule pages style
/// against.
#[derive(Default)]
pub struct Pages {
    theme: Mutex<String>,
}

impl Pages {
    fn theme(&self) -> String {
        self.theme
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    /// The page with the current theme, the base styles and the bootstrap
    /// ahead of everything the agent wrote.
    fn themed(&self, html: &str) -> String {
        let markup = format!(
            "<meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><style id=\"sikemux-theme\">{}</style><style>{BASE_CSS}</style><script>{BOOTSTRAP}</script>",
            self.theme()
        );
        insert_head(html, &markup)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PageRef {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

fn pages_dir<R: Runtime>(app: &AppHandle<R>) -> AppResult<PathBuf> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| AppError::Other(format!("pages directory unavailable: {error}")))?
        .join("pages"))
}

fn is_page_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/* Elements ahead of `<html>` land in the head the parser makes for them, so
the markup goes straight after the doctype and needs no parsing of the page. */
fn insert_head(html: &str, markup: &str) -> String {
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
    if let Some(rest) = reference.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    folder.join(reference)
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

/// Keeps the page an agent wrote at `source` and names it for the chat.
pub fn publish(
    app: &AppHandle,
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
    let dir = pages_dir(app).map_err(|error| error.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|error| format!("could not keep the page: {error}"))?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    std::fs::write(dir.join(format!("{id}.html")), page)
        .map_err(|error| format!("could not keep the page: {error}"))?;
    Ok(PageRef { id, title, height })
}

/// A loopback address showing `html` with the current theme in a desk tab,
/// kept under `name` so showing it again replaces the earlier copy.
fn tab_url(app: &AppHandle, name: &str, html: &str) -> Result<String, String> {
    let dir = pages_dir(app)
        .map_err(|error| error.to_string())?
        .join("tabs");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let path = dir.join(format!("{name}.html"));
    std::fs::write(&path, app.state::<Pages>().themed(html)).map_err(|error| error.to_string())?;
    app.state::<BrowserManager>().local_file_url(&path)
}

fn preview_name(agent_id: &str) -> String {
    format!("preview-{}", agent_id.replace(':', "-"))
}

/// Where an agent's draft shows in a desk tab, one draft per agent.
pub fn preview_url(app: &AppHandle, agent_id: &str, html: &str) -> Result<String, String> {
    tab_url(app, &preview_name(agent_id), html)
}

/* The draft's tab has done its job once the page is in the reply. */
fn close_preview(app: &AppHandle, agent_id: &str) {
    let manager = app.state::<BrowserManager>();
    let Ok(dir) = pages_dir(app) else {
        return;
    };
    let draft = dir
        .join("tabs")
        .join(format!("{}.html", preview_name(agent_id)));
    let (Ok(url), Ok(snapshot)) = (manager.local_file_url(&draft), manager.snapshot(agent_id))
    else {
        return;
    };
    for tab in snapshot.tabs.into_iter().filter(|tab| tab.url == url) {
        let _ = manager.close_tab(app, agent_id, &tab.id);
    }
}

pub fn handle<R: Runtime>(
    context: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    if context.webview_label() != "main" {
        responder.respond(status(StatusCode::FORBIDDEN));
        return;
    }
    let app = context.app_handle().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let id = request.uri().path().trim_start_matches('/');
        let page = is_page_id(id)
            .then(|| pages_dir(&app).ok())
            .flatten()
            .and_then(|dir| std::fs::read_to_string(dir.join(format!("{id}.html"))).ok());
        responder.respond(match page {
            Some(page) => Response::builder()
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .header(header::CONTENT_SECURITY_POLICY, SANDBOX)
                .header(header::CACHE_CONTROL, "no-store")
                .body(app.state::<Pages>().themed(&page).into_bytes())
                .expect("static page headers"),
            None => status(StatusCode::NOT_FOUND),
        });
    });
}

fn status(code: StatusCode) -> Response<Vec<u8>> {
    Response::builder()
        .status(code)
        .body(Vec::new())
        .expect("static status response")
}

/// The window's theme as the `:root` rule pages style against. Only custom
/// property names pass, and no value can close the rule or the style element.
fn theme_rule(dark: bool, variables: &BTreeMap<String, String>) -> String {
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

#[tauri::command]
pub fn page_theme(pages: State<'_, Pages>, dark: bool, variables: BTreeMap<String, String>) {
    *pages
        .theme
        .lock()
        .unwrap_or_else(|poison| poison.into_inner()) = theme_rule(dark, &variables);
}

#[tauri::command]
pub async fn page_publish(
    app: AppHandle,
    agent_id: Option<String>,
    path: String,
    title: String,
    height: Option<u32>,
) -> AppResult<PageRef> {
    let keeper = app.clone();
    let page = tauri::async_runtime::spawn_blocking(move || {
        publish(&keeper, Path::new(&path), &title, height)
    })
    .await
    .map_err(|error| AppError::Other(error.to_string()))?
    .map_err(AppError::Other)?;
    if let Some(agent_id) = agent_id {
        close_preview(&app, &agent_id);
    }
    Ok(page)
}

/// Opens a page the chat shows in a tab on the agent's desk.
#[tauri::command]
pub async fn page_open(app: AppHandle, agent_id: String, id: String) -> AppResult<String> {
    if !is_page_id(&id) {
        return Err(AppError::BadArg("not a page id"));
    }
    let page = std::fs::read_to_string(pages_dir(&app)?.join(format!("{id}.html")))
        .map_err(|_| AppError::Other("that page is gone".into()))?;
    let url = tab_url(&app, &id, &page).map_err(AppError::Other)?;
    app.state::<BrowserManager>()
        .open_tab(&app, &agent_id, Some(&url))
        .await
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

    #[test]
    fn theme_variables_cannot_break_out_of_the_rule() {
        let variables = BTreeMap::from([
            ("--accent".to_owned(), "#a277ff".to_owned()),
            ("--x".to_owned(), "red}</style><script>".to_owned()),
            ("color".to_owned(), "red".to_owned()),
            ("--Bad".to_owned(), "red".to_owned()),
        ]);
        let pages = Pages::default();
        *pages.theme.lock().unwrap() = theme_rule(false, &variables);
        let themed = pages.themed("<p>");
        assert!(themed.contains(":root{color-scheme:light;--accent:#a277ff;--x:red/stylescript;}"));
        assert!(!themed.contains("color:red"));
        assert!(!themed.contains("--Bad"));
    }
}
