//! The Landing page: who puts work on the project's branch, and the words the line under each
//! chat ends a turn with.

use gpui_kit::{
    div, px, AnyElement, App, Context, Entity, IntoElement, ParentElement, SharedString, Styled,
};
use sikemux_calls::board::BoardSettingsParams;
use sikemux_calls::board::Landing;
use sikemux_calls::call;
use sikemux_calls::landing::SetLandedMeansParams;
use sikemux_view::playbook::{Lands, Playbook};
use sikemux_view::turn::{Landed, LandedMeans, LandingHistory};

use crate::settings::rows;
use crate::theme::{self, Tokens, SPACE_1H, SPACE_2, TEXT_2XS_H, TEXT_XS};
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
            t!("Landing"),
            None,
            None,
            false,
            rows::note(t!("Reading…"), cx),
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
            Pick::new("agents", t!("Agents"))
                .hint(t!("They land their own work once its checks pass")),
            Pick::new("person", t!("You")).hint(t!("Nothing counts as landed until you land it")),
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
    let about = match (lands, history) {
        (Lands::Agents, None) => t!("Agents land their own work").to_owned(),
        (Lands::Person, None) => t!("You land the work agents leave").to_owned(),
        (Lands::Agents, Some(history)) => t!(
            "Agents land their own work · read from how work reached {branch} here",
            branch = history.branch
        ),
        (Lands::Person, Some(history)) => t!(
            "You land the work agents leave · read from how work reached {branch} here",
            branch = history.branch
        ),
    };
    let branch = history.map(|history| {
        rows::row_with(
            None,
            t!("Branch"),
            Some(t!("Read from Git").into()),
            div()
                .text_size(theme::ui_text(TEXT_XS))
                .text_color(Tokens::get(cx).text_primary)
                .child(history.branch.clone()),
            cx,
        )
    });
    let body = div()
        .child(rows::row_with(
            None,
            t!("Who lands"),
            Some(about.into()),
            who,
            cx,
        ))
        .children(landed)
        .children(branch)
        .child(rows::row_with(
            None,
            t!("When a turn ends, the line reads"),
            Some(t!("With the next step as a button").into()),
            done_chip(lands, cx),
            cx,
        ));
    vec![rows::section(
        t!("Landing"),
        None,
        Some(t!("Who puts work on the project's branch, and what “landed” means here. The line under each chat uses these words.").into()),
        false,
        body,
        cx,
    )]
}

const MEANS: [(Landed, &str); 3] = [
    (Landed::Committed, n!("Committed")),
    (Landed::Pushed, n!("Pushed")),
    (Landed::Merged, n!("Merged")),
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
        (Some(history), true) => t!(
            "You chose this · {history}",
            history = history_words(history)
        ),
        (None, true) => t!("You chose this").to_owned(),
        (None, false) => t!("Read from the project's history once it has commits").to_owned(),
    };
    let chosen = MEANS.iter().position(|(landed, _)| *landed == means.means);
    let (this, folder) = (this.clone(), folder.to_owned());
    let control = Segmented::words(
        "playbook-landed-means",
        MEANS
            .iter()
            .map(|(_, word)| (SharedString::from(crate::i18n::tr(word)), None))
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
                t!("What landed means was not changed"),
                |bench, _, cx| bench.notify(toast::success(t!("Landing changed")), cx),
                cx,
            );
        })
    });
    Some(rows::row_with(
        None,
        t!("Landed means"),
        Some(about.into()),
        control,
        cx,
    ))
}

/// How most of the latest changes reached the branch, as "41 of the last 50 changes were
/// pushed straight to main".
fn history_words(history: &LandingHistory) -> String {
    let branch = &history.branch;
    match (history.means, history.count, history.of) {
        (Landed::Committed, 1, 1) => t!(
            "The one change so far was committed on {branch}",
            branch = branch
        ),
        (Landed::Committed, count, of) if count == of => t!(
            "All {of} of the last changes were committed on {branch}",
            of = of,
            branch = branch
        ),
        (Landed::Committed, count, of) => t!(
            "{count} of the last {of} changes were committed on {branch}",
            count = count,
            of = of,
            branch = branch
        ),
        (Landed::Pushed, 1, 1) => t!(
            "The one change so far was pushed straight to {branch}",
            branch = branch
        ),
        (Landed::Pushed, count, of) if count == of => t!(
            "All {of} of the last changes were pushed straight to {branch}",
            of = of,
            branch = branch
        ),
        (Landed::Pushed, count, of) => t!(
            "{count} of the last {of} changes were pushed straight to {branch}",
            count = count,
            of = of,
            branch = branch
        ),
        (Landed::Merged, 1, 1) => t!(
            "The one change so far was merged into {branch}",
            branch = branch
        ),
        (Landed::Merged, count, of) if count == of => t!(
            "All {of} of the last changes were merged into {branch}",
            of = of,
            branch = branch
        ),
        (Landed::Merged, count, of) => t!(
            "{count} of the last {of} changes were merged into {branch}",
            count = count,
            of = of,
            branch = branch
        ),
        (Landed::Unknown, 1, 1) => t!(
            "The one change so far was brought to {branch}",
            branch = branch
        ),
        (Landed::Unknown, count, of) if count == of => t!(
            "All {of} of the last changes were brought to {branch}",
            of = of,
            branch = branch
        ),
        (Landed::Unknown, count, of) => t!(
            "{count} of the last {of} changes were brought to {branch}",
            count = count,
            of = of,
            branch = branch
        ),
    }
}

fn lands_word(lands: Lands) -> &'static str {
    match lands {
        Lands::Agents => t!("Agents"),
        Lands::Person => t!("You"),
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
        Lands::Agents => t!("Done · landed · checks pass"),
        Lands::Person => t!("Done · committed · checks pass"),
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
        t!("Who lands was not changed"),
        |bench, _, cx| bench.notify(toast::success(t!("Landing changed")), cx),
        cx,
    );
}
