//! Sikemux's own links, followed inside this app: `sikemux://agent/<agent>/<session>?project=`
//! takes an agent's chat up again in its project on this host, as the Mac does. Any message
//! can hold one, so a folder that is not already a project is opened only once the person
//! agrees. `sikemux://slice/<card>?repo=<owner/name>`, which a pull request's description
//! holds, opens that slice in the project whose code host repository it names.

use std::path::PathBuf;

use gpui_kit::{App, Context, Entity, Window};
use url::Url;

use crate::core;
use crate::toast;
use crate::workbench::Workbench;
use sikemux_core::server::call;
use sikemux_core::server::workspace::SessionChoice;

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

/// A slice a pull request's link names: its card, and the repository on the code host.
fn slice_of(link: &str) -> Option<(u64, String)> {
    let url = Url::parse(link).ok()?;
    if url.scheme() != "sikemux" || url.host_str() != Some("slice") {
        return None;
    }
    let card = url.path().trim_matches('/').parse().ok()?;
    let repo = (url.query_pairs())
        .find(|(key, _)| key == "repo")
        .map(|(_, value)| value.into_owned())?;
    Some((card, repo))
}

/// Opens the slice in the first project whose repository is the one named.
fn open_slice(bench: &mut Workbench, card: u64, repo: &str, cx: &mut Context<Workbench>) -> bool {
    let project = bench.this_view().and_then(|view| {
        (view.workspace.projects.iter())
            .find(|project| {
                project
                    .code_host
                    .as_ref()
                    .is_some_and(|host| host.repo == repo)
            })
            .map(|project| project.path.to_string_lossy().into_owned())
    });
    let Some(project) = project else {
        return false;
    };
    bench.open_slice(&project, card, cx);
    true
}

/// Follows a `sikemux://` link. A link this app does not understand goes nowhere, rather
/// than to whichever app the system would pick, and the window says so.
pub fn follow(bench: &mut Workbench, link: &str, cx: &mut Context<Workbench>) {
    if let Some((card, repo)) = slice_of(link) {
        if !open_slice(bench, card, &repo, cx) {
            bench.notify(toast::error(format!("No project here is {repo}")), cx);
        }
        return;
    }
    let resume = parse(link).filter(|resume| {
        bench
            .launchers()
            .iter()
            .any(|launcher| launcher.provider == resume.provider)
    });
    let Some(resume) = resume else {
        return bench.notify(toast::error(format!("Sikemux couldn't open {link}")), cx);
    };
    let open = bench.this_view().and_then(|view| {
        view.chats
            .iter()
            .find(|chat| {
                chat.provider == resume.provider
                    && chat.session_id.as_deref() == Some(resume.session.as_str())
            })
            .map(|chat| chat.agent_id.clone())
    });
    if let Some(agent) = open {
        bench.open_agent(&agent, cx);
        return;
    }
    let known = bench.this_view().is_some_and(|view| {
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
            let Ok(added) = bench.update(cx, |bench, cx| bench.add_project(&path, cx)) else {
                return;
            };
            if added.await.is_none() {
                return;
            }
        }
        let taken = core::Target::here(&asks)
            .request_typed::<call::ResumeSession>(SessionChoice {
                provider: resume.provider.clone(),
                session_id: resume.session.clone(),
                project: path.clone().into(),
                cwd: None,
                title: None,
            })
            .await;
        let _ = bench.update(cx, |bench, cx| match taken {
            Ok(taken) => {
                bench.open_agent(&taken.agent_id, cx);
            }
            Err(error) => bench.notify(toast::error(format!("The chat did not open: {error}")), cx),
        });
    })
    .detach();
}
