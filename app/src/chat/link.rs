//! A link in a message: a file the agent named shows in Finder, or in the editor when the
//! agent runs on another host; a web page opens on the agent's desk, a pull request there as
//! the Git tab shows one, or with ⌘ held where the system opens it; Sikemux's own links are followed here. Any other link, or a web
//! address carrying a password, goes nowhere, so a link an agent wrote cannot start another app.

use std::path::PathBuf;

use gpui_kit::{App, WeakEntity, Window};
use url::Url;

use crate::icons::Icon;
use crate::workbench::Workbench;

/// The mark of the service a web link leads to, when a plugin speaks for it.
pub fn service_mark(href: &str) -> Option<Icon> {
    let url = Url::parse(href).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?;
    let on = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    Some(if on("github.com") {
        Icon::Github
    } else if on("bitbucket.org") {
        Icon::Bitbucket
    } else if on("atlassian.net") {
        Icon::Jira
    } else {
        return None;
    })
}

/// The file a link names, when it names one on this host.
pub fn local_path(href: &str) -> Option<PathBuf> {
    if href.starts_with('/') {
        return Some(PathBuf::from(href));
    }
    let url = Url::parse(href).ok().filter(|url| url.scheme() == "file")?;
    url.to_file_path().ok()
}

pub fn open(href: &str, agent: &str, bench: &WeakEntity<Workbench>, window: &Window, cx: &mut App) {
    if href.starts_with("sikemux://") {
        bench
            .update(cx, |bench, cx| crate::links::follow(bench, href, cx))
            .ok();
        return;
    }
    if let Some(path) = local_path(href) {
        let elsewhere = bench.upgrade().is_some_and(|bench| {
            *bench.read(cx).chat_target(agent).place() != crate::hosts::Place::This
        });
        if elsewhere {
            bench
                .update(cx, |bench, cx| bench.open_file(&path, cx))
                .ok();
        } else {
            cx.reveal_path(&path);
        }
        return;
    }
    let web = Url::parse(href).ok().is_some_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.username().is_empty()
            && url.password().is_none()
    });
    if !web {
        return;
    }
    if !window.modifiers().platform {
        let shown = bench
            .update(cx, |bench, cx| bench.open_link_on_desk(agent, href, cx))
            .unwrap_or(false);
        if shown {
            return;
        }
    }
    cx.open_url(href);
}
