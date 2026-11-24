//! Installing a mod from GitHub, as a `sikemux://install-mod?repo=<owner>/<name>` link asks, such as
//! sikemux.com's Mods page has: the core reads the repository's `mod.json`, this sheet says what
//! the mod reads, changes and reaches, and nothing is downloaded until the person installs it.

use std::rc::Rc;

use gpui_kit::{div, AnyElement, App, Context, Div, IntoElement, ParentElement, Styled};
use sikemux_calls::call;
use sikemux_calls::mods::{github_repo, InstallRepoParams, RepoMod, RepoParams};
use url::Url;

use crate::icons::Icon;
use crate::theme::{self, Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_3, TEXT_SM, TEXT_XS};
use crate::toast;
use crate::ui::{self, dialog};
use crate::workbench::Workbench;

/// The mod the sheet asks about, while it is open.
pub struct Installing {
    found: RepoMod,
    working: bool,
}

/// The repository a `sikemux://install-mod` link names, by `repo=<owner>/<name>` or by
/// `url=https://github.com/<owner>/<name>`.
pub fn link(link: &str) -> Option<String> {
    let url = Url::parse(link).ok()?;
    if url.scheme() != "sikemux" || url.host_str() != Some("install-mod") {
        return None;
    }
    let (_, named) = url
        .query_pairs()
        .find(|(key, _)| key == "repo" || key == "url")?;
    github_repo(&named)
}

impl Workbench {
    /// Reads what the mod in `repo` says of itself on the host in front, then asks the person.
    pub fn offer_repo_mod(&mut self, repo: String, cx: &mut Context<Self>) {
        self.notify(
            toast::info(t!("Reading {repo} on GitHub…", repo = repo)),
            cx,
        );
        let read = self
            .target()
            .request_typed::<call::PreviewRepoMod>(RepoParams { repo });
        self.report(
            read,
            t!("The mod could not be read"),
            |bench, found, cx| {
                bench.overlays.installing_mod = Some(Installing {
                    found,
                    working: false,
                });
                cx.notify();
            },
            cx,
        );
    }

    fn close_install(&mut self, cx: &mut Context<Self>) {
        self.overlays.installing_mod = None;
        cx.notify();
    }

    fn install_repo_mod(&mut self, cx: &mut Context<Self>) {
        let Some(installing) = self.overlays.installing_mod.as_mut() else {
            return;
        };
        if installing.working {
            return;
        }
        installing.working = true;
        let found = installing.found.clone();
        let installed = self
            .target()
            .request_typed::<call::InstallRepoMod>(InstallRepoParams {
                repo: found.repo.clone(),
                commit: found.commit.clone(),
            });
        cx.spawn(async move |bench, cx| {
            let answer = installed.await;
            let _ = bench.update(cx, |bench, cx| {
                bench.overlays.installing_mod = None;
                match answer {
                    Ok(id) => {
                        let done = t!("{name} is installed", name = found.name);
                        bench.notify(toast::success(done), cx);
                        bench.open_mod(&id, cx);
                    }
                    Err(error) => bench.notify(
                        toast::error(t!(
                            "{name} did not install: {error}",
                            name = found.name,
                            error = error
                        ))
                        .autohide(false),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// One group of the sheet: its label in spaced capitals, then a line for each thing in it.
fn group(label: &str, lines: Vec<(Icon, String)>, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .child(ui::caps(label, cx))
        .children(lines.into_iter().map(|(icon, words)| {
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .text_size(theme::ui_text(TEXT_SM))
                .text_color(t.text_secondary)
                .child(ui::icon(icon, 13.).text_color(t.text_tertiary).flex_none())
                .child(words)
        }))
}

/// A connector's own mark for what a mod reads, such as `github`, else an eye.
fn read_icon(name: &str) -> Icon {
    let head = name.split('.').next().unwrap_or(name);
    match head {
        "git" => Icon::Git,
        _ => Icon::named(head).unwrap_or(Icon::Eye),
    }
}

pub fn view(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let installing = bench.overlays.installing_mod.as_ref()?;
    let found = &installing.found;
    let t = Tokens::get(cx).clone();
    let nothing = || vec![(Icon::Check, t!("Nothing").to_owned())];
    let reads: Vec<_> = found
        .reads
        .iter()
        .map(|name| (read_icon(name), name.clone()))
        .collect();
    let mut writes: Vec<_> = (found.writes.iter())
        .map(|name| (Icon::Pencil, name.clone()))
        .collect();
    if !found.collections.is_empty() {
        writes.push((
            Icon::Database,
            t!(
                "Its own data: {names}",
                names = found.collections.join(", ")
            ),
        ));
    }
    let reaches: Vec<_> = (found.connect.iter())
        .map(|origin| (Icon::Globe, origin.clone()))
        .collect();
    let short: String = found.commit.chars().take(7).collect();
    let source = div()
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .text_size(theme::ui_text(TEXT_XS))
        .text_color(t.text_tertiary)
        .child(ui::icon(Icon::Github, 12.))
        .child(t!(
            "github.com/{repo} at {commit}",
            repo = found.repo,
            commit = short
        ));
    let asks = !found.writes.is_empty() || !found.connect.is_empty();
    let note = match asks {
        true => t!("Runs in its own pane, and asks before it changes or reaches anything. Your sign-ins never reach it."),
        false => t!("Runs in its own pane. Your sign-ins never reach it."),
    };
    let body = dialog::marked_body()
        .gap(SPACE_3)
        .child(source)
        .children(
            (!found.description.is_empty()).then(|| dialog::prose(found.description.clone(), &t)),
        )
        .child(group(
            t!("It reads"),
            match reads.is_empty() {
                true => nothing(),
                false => reads,
            },
            cx,
        ))
        .child(group(
            t!("It writes"),
            match writes.is_empty() {
                true => nothing(),
                false => writes,
            },
            cx,
        ))
        .children((!reaches.is_empty()).then(|| group(t!("It reaches"), reaches, cx)))
        .child(
            div()
                .text_size(theme::ui_text(TEXT_XS))
                .text_color(t.text_tertiary)
                .child(note),
        )
        .into_any_element();
    let this = cx.entity();
    let close: dialog::Answer = {
        let this = this.clone();
        Rc::new(move |_, cx| this.update(cx, |bench, cx| bench.close_install(cx)))
    };
    let install: dialog::Answer = {
        let this = this.clone();
        Rc::new(move |_, cx| this.update(cx, |bench, cx| bench.install_repo_mod(cx)))
    };
    let code = format!("https://github.com/{}/tree/{}", found.repo, found.commit);
    let read_code: dialog::Answer = Rc::new(move |_, cx| cx.open_url(&code));
    let label = match (installing.working, found.replaces) {
        (true, _) => t!("Installing…"),
        (false, true) => t!("Update"),
        (false, false) => t!("Install"),
    };
    let foot = dialog::hinted_foot(dialog::escape(t!("cancel"), cx), cx)
        .child(dialog::button(
            "install-mod-code",
            t!("Read code").into(),
            None,
            read_code,
            cx,
        ))
        .child(dialog::button(
            "install-mod-cancel",
            t!("Cancel").into(),
            None,
            close.clone(),
            cx,
        ))
        .child(dialog::button(
            "install-mod-yes",
            label.into(),
            Some(t.accent),
            install.clone(),
            cx,
        ));
    let title = t!("Install {name}?", name = found.name);
    let tile = crate::mods::icon_named(found.icon.as_deref());
    Some(
        dialog::marked_sheet(tile, title.into(), body, foot, install, close, cx).into_any_element(),
    )
}
