//! Shows the pages the core keeps for agents: the chat over `page://` in a
//! sandboxed frame, so their scripts never reach the app, and a desk tab as a
//! copy over loopback.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use sikemux_process::sync::Locked;
use tauri::http::{header, Request, Response, StatusCode};
use tauri::{AppHandle, Manager, Runtime, State, UriSchemeContext, UriSchemeResponder};

use crate::browser::BrowserManager;
use crate::error::{AppError, AppResult};

pub const SCHEME: &str = "page";

/* What a page may do in its frame: run its own scripts and forms, nothing that
reaches the app, its storage or another window. */
const SANDBOX: &str = "sandbox allow-scripts allow-forms";

const BASE_CSS: &str = "html{background:var(--background,Canvas);color:var(--foreground,CanvasText);font-family:var(--font-sans,system-ui);font-size:14px;line-height:1.5;-webkit-font-smoothing:antialiased;-webkit-text-size-adjust:100%}html[data-sikemux-framed]{background:transparent;overflow:hidden!important}body{margin:0}code,kbd,pre,samp{font-family:var(--font-mono,ui-monospace)}";

/* Speaks the MCP Apps messages (JSON-RPC over postMessage) between a framed
page and the chat: the chat sends theme changes, the page reports its height
and hands over the links the person clicks. A wheel turned over the page that
nothing in it can scroll goes to the chat too, which never sees it otherwise. */
const BOOTSTRAP: &str = r#"(()=>{const root=document.documentElement;if(window.parent===window)return;root.setAttribute("data-sikemux-framed","");const theme=document.getElementById("sikemux-theme");const post=(message)=>window.parent.postMessage({jsonrpc:"2.0",...message},"*");let links=0;window.addEventListener("message",(event)=>{const data=event.data;if(event.source!==window.parent||!data||data.jsonrpc!=="2.0"||data.method!=="ui/notifications/host-context-changed")return;const variables=data.params&&data.params.styles&&data.params.styles.variables;if(!variables||typeof variables!=="object")return;let css=":root{color-scheme:"+(data.params.theme==="light"?"light":"dark")+";";for(const name in variables)if(/^--[a-z0-9-]+$/.test(name))css+=name+":"+String(variables[name]).replace(/[;{}<>]/g,"")+";";theme.textContent=css+"}";});document.addEventListener("click",(event)=>{const link=event.isTrusted&&event.composedPath().find((target)=>target instanceof Element&&target.matches("a[href]"));if(!link)return;let url;try{url=new URL(link.getAttribute("href"),document.baseURI);}catch{return;}if(!/^https?:$/.test(url.protocol))return;event.preventDefault();post({id:"sikemux-link-"+(++links),method:"ui/open-link",params:{url:url.href}});},true);let reported=0;const report=()=>{const height=Math.ceil(root.getBoundingClientRect().height);if(height===reported)return;reported=height;post({method:"ui/notifications/size-changed",params:{height}});};const observer=new ResizeObserver(report);observer.observe(root);document.addEventListener("DOMContentLoaded",()=>{if(document.body)observer.observe(document.body);report();});window.addEventListener("load",report);const scrollsItself=(node,down)=>{for(;node instanceof Element;node=node.parentElement){const style=getComputedStyle(node).overflowY;const scroller=(style==="auto"||style==="scroll")&&node!==document.body&&node!==root;if(scroller&&(down?node.scrollTop+node.clientHeight<node.scrollHeight-1:node.scrollTop>0))return true;}return false;};window.addEventListener("wheel",(event)=>{if(event.ctrlKey||Math.abs(event.deltaY)<=Math.abs(event.deltaX)||scrollsItself(event.target,event.deltaY>0))return;event.preventDefault();const scale=event.deltaMode===1?16:event.deltaMode===2?window.innerHeight:1;post({method:"sikemux/wheel",params:{deltaY:event.deltaY*scale}});},{passive:false});})();"#;

/// The theme the window last handed over, as the `:root` rule pages style
/// against.
#[derive(Default)]
pub struct Pages {
    theme: Mutex<String>,
}

impl Pages {
    fn theme(&self) -> String {
        self.theme.locked().clone()
    }

    /// The page with the current theme, the base styles and the bootstrap
    /// ahead of everything the agent wrote.
    fn themed(&self, html: &str) -> String {
        let markup = format!(
            "<meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><style id=\"sikemux-theme\">{}</style><style>{BASE_CSS}</style><script>{BOOTSTRAP}</script>",
            self.theme()
        );
        sikemux_core::pages::insert_head(html, &markup)
    }
}

fn pages_dir<R: Runtime>(app: &AppHandle<R>) -> AppResult<PathBuf> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| AppError::Other(format!("pages directory unavailable: {error}")))?
        .join(sikemux_core::pages::FOLDER))
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
        let page = sikemux_core::pages::is_page_id(id)
            .then(|| pages_dir(&app).ok())
            .flatten()
            .and_then(|dir| std::fs::read_to_string(dir.join(format!("{id}.html"))).ok());
        let response = match page {
            Some(page) => Response::builder()
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .header(header::CONTENT_SECURITY_POLICY, SANDBOX)
                .header(header::CACHE_CONTROL, "no-store")
                .body(app.state::<Pages>().themed(&page).into_bytes())
                .expect("static page headers"),
            None => status(StatusCode::NOT_FOUND),
        };
        crate::preview::respond_on_main_thread(&app, responder, response);
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
    *pages.theme.locked() = theme_rule(dark, &variables);
}

/// The core kept a page the agent showed: the draft's tab has done its job,
/// and an agent in a terminal sees the page on its desk.
#[tauri::command]
pub async fn page_shown(app: AppHandle, agent_id: String, id: String, desk: bool) -> AppResult<()> {
    close_preview(&app, &agent_id);
    if desk {
        page_open(app, agent_id, id).await?;
    }
    Ok(())
}

/// Opens a page the chat shows in a tab on the agent's desk.
async fn page_open(app: AppHandle, agent_id: String, id: String) -> AppResult<String> {
    if !sikemux_core::pages::is_page_id(&id) {
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
