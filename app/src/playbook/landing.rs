//! The Landing page: who puts work on the project's branch, and the words the line under each
//! chat ends a turn with.

use gpui_kit::{
    div, px, AnyElement, App, Context, Entity, IntoElement, ParentElement, SharedString, Styled,
};
use sikemux_core::server::board::{BoardSettingsParams, Landing};
use sikemux_core::server::call;
use sikemux_core::server::landing::SetLandedMeansParams;
use sikemux_view::playbook::{Lands, Playbook};
use sikemux_view::turn::{Landed, LandedMeans, LandingHistory};

use crate::settings::rows;
use crate::theme::{self, Tokens, SPACE_1H, SPACE_2, TEXT_2XS_H};
use crate::toast;
use crate::ui::{select, Pick, Segmented};
use crate::workbench::Workbench;

pub(super) fn page(
    playbook: Option<&Playbook>,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let Some(playbook) = playbook else {
        return vec![rows::section(
            "Landing",
            None,
            None,
            false,
            rows::note("Reading…", cx),
            cx,
        )];
    };
    let lands = playbook.landing.lands;
    let means = playbook.landing.means.as_ref();
    let history = means.and_then(|means| means.history.as_ref());
    let landed = means_row(means, folder, this, cx);
    let (this, folder) = (this.clone(), folder.to_owned());
    let who = select(
        "playbook-lands",
        lands_word(lands),
        vec![
            Pick::new("agents", "Agents").hint("They land their own work once its checks pass"),
            Pick::new("person", "You").hint("Nothing counts as landed until you land it"),
        ],
        Some(lands_key(lands).into()),
        false,
        move |key, _, cx| {
            let landing = match key.as_ref() {
                "person" => Landing::Person,
                _ => Landing::Core,
            };
            let folder = folder.clone();
            this.update(cx, |bench, cx| set_landing(bench, folder, landing, cx));
        },
        cx,
    );
    let mut about = match lands {
        Lands::Agents => "Agents land their own work".to_owned(),
        Lands::Person => "You land the work agents leave".to_owned(),
    };
    if let Some(history) = history {
        about.push_str(&format!(
            " · read from how work reached {} here",
            history.branch
        ));
    }
    let branch = history.map(|history| {
        rows::row_with(
            None,
            "Branch",
            Some(format!("{} · from Git", history.branch).into()),
            select(
                "playbook-branch",
                history.branch.clone(),
                Vec::new(),
                None,
                true,
                |_, _, _| {},
                cx,
            ),
            cx,
        )
    });
    let body = div()
        .child(rows::row_with(
            None,
            "Who lands",
            Some(about.into()),
            who,
            cx,
        ))
        .children(landed)
        .children(branch)
        .child(rows::row_with(
            None,
            "When a turn ends, the line reads",
            Some("With the next step as a button".into()),
            done_chip(lands, cx),
            cx,
        ));
    vec![rows::section(
        "Landing",
        None,
        Some("Who puts work on the project's branch, and what “landed” means here. The line under each chat uses these words.".into()),
        false,
        body,
        cx,
    )]
}

const MEANS: [(Landed, &str); 3] = [
    (Landed::Committed, "Committed"),
    (Landed::Pushed, "Pushed"),
    (Landed::Merged, "Merged"),
];

/// What counts as landed, with how the latest changes reached the branch under it.
fn means_row(
    means: Option<&LandedMeans>,
    folder: &str,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Option<gpui_kit::Div> {
    let means = means?;
    let about = match (&means.history, means.chosen) {
        (Some(history), false) => history_words(history),
        (Some(history), true) => format!("You chose this · {}", history_words(history)),
        (None, true) => "You chose this".to_owned(),
        (None, false) => "Read from the project's history once it has commits".to_owned(),
    };
    let chosen = MEANS.iter().position(|(landed, _)| *landed == means.means);
    let (this, folder) = (this.clone(), folder.to_owned());
    let control = Segmented::words(
        "playbook-landed-means",
        MEANS
            .iter()
            .map(|(_, word)| (SharedString::from(*word), None))
            .collect(),
        chosen.unwrap_or(usize::MAX),
    )
    .on_select(move |at, _, cx| {
        let Some((landed, _)) = MEANS.get(*at) else {
            return;
        };
        let (folder, landed) = (folder.clone(), *landed);
        this.update(cx, |bench, cx| {
            let asked =
                bench
                    .target()
                    .request_typed::<call::SetLandedMeans>(SetLandedMeansParams {
                        project: folder,
                        means: Some(landed),
                    });
            bench.report(
                asked,
                "What landed means was not changed",
                |bench, _, cx| bench.notify(toast::success("Landing changed"), cx),
                cx,
            );
        })
    });
    Some(rows::row_with(
        None,
        "Landed means",
        Some(about.into()),
        control,
        cx,
    ))
}

/// How most of the latest changes reached the branch, as "41 of the last 50 changes were
/// pushed straight to main".
fn history_words(history: &LandingHistory) -> String {
    let how = match history.means {
        Landed::Committed => "committed on",
        Landed::Pushed => "pushed straight to",
        Landed::Merged => "merged into",
        Landed::Unknown => "brought to",
    };
    match (history.count, history.of) {
        (1, 1) => format!("The one change so far was {how} {}", history.branch),
        (count, of) if count == of => {
            format!("All {of} of the last changes were {how} {}", history.branch)
        }
        (count, of) => format!(
            "{count} of the last {of} changes were {how} {}",
            history.branch
        ),
    }
}

fn lands_word(lands: Lands) -> &'static str {
    match lands {
        Lands::Agents => "Agents",
        Lands::Person => "You",
    }
}

fn lands_key(lands: Lands) -> &'static str {
    match lands {
        Lands::Agents => "agents",
        Lands::Person => "person",
    }
}

/// The state line's words for a turn that ended well, as this project's landing has it.
fn done_chip(lands: Lands, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let words = match lands {
        Lands::Agents => "Done · landed · checks pass",
        Lands::Person => "Done · committed · checks pass",
    };
    div()
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .px(SPACE_2)
        .py(px(3.))
        .rounded(theme::RADIUS_1)
        .bg(t.surface_sunken)
        .border_1()
        .border_color(t.line_soft)
        .text_size(theme::ui_text(TEXT_2XS_H))
        .text_color(t.text_secondary)
        .child(div().size(px(6.)).rounded_full().bg(t.accent))
        .child(words)
        .into_any_element()
}

fn set_landing(
    bench: &mut Workbench,
    folder: String,
    landing: Landing,
    cx: &mut Context<Workbench>,
) {
    let asked = bench
        .target()
        .request_typed::<call::BoardSettings>(BoardSettingsParams {
            project: folder,
            landing: Some(landing),
            ..BoardSettingsParams::default()
        });
    bench.report(
        asked,
        "Who lands was not changed",
        |bench, _, cx| bench.notify(toast::success("Landing changed"), cx),
        cx,
    );
}
