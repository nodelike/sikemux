//! A page an agent showed, drawn inline in its chat reply, or an HTML file shown as it
//! reads: a web view of its own that keeps no storage and never navigates. Its script can
//! only report the page's height, a link the person clicked and a scroll the page had no
//! use for; the app decides what each means.

use std::path::Path;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
pub use linux::PageView;
#[cfg(target_os = "macos")]
pub use macos::PageView;

/// Sizes the view to the page and hands over the links and scrolls the person meant.
pub(crate) const BOOTSTRAP: &str = r#"(()=>{const post=(message)=>window.webkit.messageHandlers.sikemux.postMessage(JSON.stringify(message));const root=document.documentElement;root.setAttribute("data-sikemux-framed","");document.addEventListener("click",(event)=>{const link=event.isTrusted&&event.composedPath().find((target)=>target instanceof Element&&target.matches("a[href]"));if(!link)return;let url;try{url=new URL(link.getAttribute("href"),document.baseURI);}catch{return;}if(!/^https?:$/.test(url.protocol))return;event.preventDefault();post({kind:"link",url:url.href});},true);let reported=0;const report=()=>{const height=Math.ceil(root.getBoundingClientRect().height);if(height===reported)return;reported=height;post({kind:"height",height});};const observer=new ResizeObserver(report);observer.observe(root);document.addEventListener("DOMContentLoaded",()=>{if(document.body)observer.observe(document.body);report();});window.addEventListener("load",report);const scrollsItself=(node,down)=>{for(;node instanceof Element;node=node.parentElement){const style=getComputedStyle(node).overflowY;const scroller=(style==="auto"||style==="scroll")&&node!==document.body&&node!==root;if(scroller&&(down?node.scrollTop+node.clientHeight<node.scrollHeight-1:node.scrollTop>0))return true;}return false;};window.addEventListener("wheel",(event)=>{if(event.ctrlKey||Math.abs(event.deltaY)<=Math.abs(event.deltaX)||scrollsItself(event.target,event.deltaY>0))return;event.preventDefault();const scale=event.deltaMode===1?16:event.deltaMode===2?window.innerHeight:1;post({kind:"wheel",deltaY:event.deltaY*scale});},{passive:false});})();"#;

/// Where a page is drawn.
#[derive(Clone, Copy)]
pub enum Room<'a> {
    /// In a reply, at its full height on the app's ground, scrolled with what holds it.
    Grows,
    /// Filling a file's pane on a page's usual white and scrolling itself. With the file's
    /// path on this disk, what it links to beside it, such as its styles and pictures, loads too.
    Fills(Option<&'a Path>),
}

/// The address a page's text is read from, which is the only one it may load.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(crate) fn loads_as(base: Option<&str>) -> &str {
    base.unwrap_or("about:blank")
}

/// Whether the page may go to `url`: its own first load, or a link to a place within it.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(crate) fn stays(url: &str, loaded: &str, first_load: bool) -> bool {
    let (page, place) = url.split_once('#').unwrap_or((url, ""));
    page == loaded && (first_load || !place.is_empty())
}

/// What a page's script reported.
pub enum PageEvent {
    Height(f64),
    /// A link the person clicked, which the page itself never opens.
    Link(String),
    /// A vertical scroll the page did not use, for whatever holds the page to scroll by.
    Wheel(f64),
}

impl PageEvent {
    /// The event a message from the page's script names.
    pub(crate) fn read(text: &str) -> Option<Self> {
        let message = serde_json::from_str::<serde_json::Value>(text).ok()?;
        match message["kind"].as_str()? {
            "height" => message["height"].as_f64().map(PageEvent::Height),
            "link" => message["url"]
                .as_str()
                .map(|url| PageEvent::Link(url.to_owned())),
            "wheel" => message["deltaY"].as_f64().map(PageEvent::Wheel),
            _ => None,
        }
    }
}
