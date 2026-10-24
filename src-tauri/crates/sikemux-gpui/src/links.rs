//! Sikemux's own links, followed inside this app: `sikemux://agent/<agent>/<session>?project=`
//! takes an agent's chat up again in its project on this host, as the Mac does. Any message
//! can hold one, so a folder that is not already a project is opened only once the person
//! agrees.

use std::path::PathBuf;

use gpui_kit::{Context, Window};
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

/// Follows a `sikemux://` link, and says whether it was one this app understands. A link it
/// does not understand goes nowhere, rather than to whichever app the system would pick.
pub fn follow(bench: &mut Workbench, link: &str, cx: &mut Context<Workbench>) -> bool {
    let Some(resume) = parse(link) else {
        return false;
    };
    let launchable = bench
        .launchers()
        .iter()
        .any(|launcher| launcher.provider == resume.provider);
    if !launchable {
        return false;
    }
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
        return true;
    }
    let known = bench.view.as_ref().is_some_and(|view| {
        view.workspace
            .projects
            .iter()
            .any(|project| project.path == resume.project)
    });
    if known {
        take_up(bench, resume, false, cx);
        return true;
    }
    let body = format!(
        "A link asks to resume a {} chat in {}, which is not one of your projects.\nThe agent starts there at once and follows the folder's own settings and hooks. Go ahead only if you trust it.",
        agent_name(bench, &resume.provider),
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
    true
}

fn agent_name(bench: &Workbench, provider: &str) -> String {
    bench
        .launchers()
        .iter()
        .find(|launcher| launcher.provider == provider)
        .map_or_else(|| provider.to_owned(), |launcher| launcher.label.clone())
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
