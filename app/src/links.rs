//! Sikemux's own links, followed inside this app: `sikemux://agent/<agent>/<session>?project=`
//! takes an agent's chat up again in its project on this host, as the Mac does. Any message
//! can hold one, so a folder that is not already a project is opened only once the person
//! agrees.

use std::path::PathBuf;

use gpui_kit::{App, Context, Entity, Window};
use serde_json::json;
use url::Url;

use crate::core;
use crate::toast;
use crate::workbench::Workbench;

/// An agent's chat a link asks to take up again.
struct Resume {
    provider: String,
    session: String,
    project: PathBuf,
}

fn parse(link: &str) -> Option<Resume> {
    let url = Url::parse(link).ok()?;
    if url.scheme() != "sikemux" || url.host_str() != Some("agent") {
        return None;
    }
    let mut parts = url
        .path_segments()?
        .filter(|part| !part.is_empty())
        .map(|part| {
            percent_encoding::percent_decode_str(part)
                .decode_utf8_lossy()
                .into_owned()
        });
    let (provider, session) = (parts.next()?, parts.next()?);
    let project = url
        .query_pairs()
        .find(|(key, _)| key == "project")
        .map(|(_, value)| PathBuf::from(value.into_owned()))
        .filter(|project| project.is_absolute())?;
    let plain = |text: &str| {
        !text.is_empty()
            && text
                .chars()
                .all(|letter| letter.is_ascii_alphanumeric() || "-_.:".contains(letter))
    };
    (plain(&provider) && plain(&session)).then_some(Resume {
        provider,
        session,
        project,
    })
}

/// Takes what the system hands this app to open: each `sikemux://` link, such as one clicked
/// in another app, brings the window to the front for it, and files and folders, such as
/// from Finder's Open With, open as `sikemux open` would; a start with nothing to open, as
/// Linux hands over, brings the window forward. What is handed over while the app is still
/// opening waits for its window.
pub fn follow_opened(
    bench: Entity<Workbench>,
    mut opened: tokio::sync::mpsc::UnboundedReceiver<Vec<String>>,
    cx: &mut App,
) {
    cx.spawn(async move |cx| {
        while let Some(urls) = opened.recv().await {
            let (files, links): (Vec<_>, Vec<_>) =
                urls.into_iter().partition(|url| url.starts_with("file:"));
            let paths = files
                .iter()
                .filter_map(|file| Url::parse(file).ok()?.to_file_path().ok())
                .collect();
            bench.update(cx, |bench, cx| {
                let forward = files.is_empty() || !links.is_empty();
                bench.open_handed_paths(paths, cx);
                for link in links {
                    follow(bench, &link, cx);
                }
                if forward {
                    bench.bring_window_forward(cx);
                }
            });
        }
    })
    .detach();
}

/// Follows a `sikemux://` link. A link this app does not understand goes nowhere, rather
/// than to whichever app the system would pick, and the window says so.
pub fn follow(bench: &mut Workbench, link: &str, cx: &mut Context<Workbench>) {
    let resume = parse(link).filter(|resume| {
        bench
            .launchers()
            .iter()
            .any(|launcher| launcher.provider == resume.provider)
    });
    let Some(resume) = resume else {
        return bench.notify(toast::error(format!("Sikemux couldn't open {link}")), cx);
    };
    let open = bench.view.as_ref().and_then(|view| {
        view.chats
            .iter()
            .find(|chat| {
                chat.provider == resume.provider
                    && chat.session_id.as_deref() == Some(resume.session.as_str())
            })
            .map(|chat| chat.agent_id.clone())
    });
    if let Some(agent) = open {
        bench.open_agent(&agent);
        return;
    }
    let known = bench.view.as_ref().is_some_and(|view| {
        view.workspace
            .projects
            .iter()
            .any(|project| project.path == resume.project)
    });
    if known {
        take_up(bench, resume, false, cx);
        return;
    }
    let body = format!(
        "A link asks to resume a {} chat in {}, which is not one of your projects.\nThe agent starts there at once and follows the folder's own settings and hooks. Go ahead only if you trust it.",
        bench.agent_name(&resume.provider),
        resume.project.display(),
    );
    let resume = std::rc::Rc::new(resume);
    bench.confirm(
        "Open this folder as a project?",
        body,
        "Open project",
        false,
        move |bench, yes, _: &mut Window, cx| {
            if yes {
                let resume = Resume {
                    provider: resume.provider.clone(),
                    session: resume.session.clone(),
                    project: resume.project.clone(),
                };
                take_up(bench, resume, true, cx);
            }
        },
        cx,
    );
}

/// Takes the chat up again on this host's core, opening its folder as a project first when
/// asked, then brings its tab forward.
fn take_up(bench: &mut Workbench, resume: Resume, add: bool, cx: &mut Context<Workbench>) {
    let asks = bench.asks.clone();
    let path = resume.project.to_string_lossy().into_owned();
    cx.spawn(async move |bench, cx| {
        if add {
            if let Err(error) = core::request(&asks, "addProject", json!({ "path": path })).await {
                let note = toast::error(format!("The folder did not open as a project: {error}"));
                let _ = bench.update(cx, |bench, cx| bench.notify(note, cx));
                return;
            }
        }
        let asked = json!({
            "provider": resume.provider,
            "sessionId": resume.session,
            "project": path,
        });
        let taken = core::request(&asks, "resumeSession", asked).await;
        let _ = bench.update(cx, |bench, cx| match taken {
            Ok(taken) => {
                if let Some(agent) = taken["agentId"].as_str() {
                    bench.open_agent(agent);
                }
            }
            Err(error) => bench.notify(toast::error(format!("The chat did not open: {error}")), cx),
        });
    })
    .detach();
}

/// Follows a link in a page outside a chat, such as an issue's comment or a release's notes:
/// with ⌘ held, or anything but a web address, as the system does; else as
/// [`Workbench::follow_web_link`] does, a pull request or issue in its project's Git tab.
pub fn web_opener(bench: gpui_kit::WeakEntity<Workbench>) -> gpui_transcript::OnLink {
    std::rc::Rc::new(move |href: &str, window: &mut Window, cx: &mut App| {
        let web = href.starts_with("https://") || href.starts_with("http://");
        if !web || window.modifiers().platform {
            cx.open_url(href);
            return;
        }
        let followed = bench.update(cx, |bench, cx| bench.follow_web_link(href, cx));
        if followed.is_err() {
            cx.open_url(href);
        }
    })
}
