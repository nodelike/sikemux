//! The Landing page: who puts work on the project's branch, and the words the line under each
//! chat ends a turn with.

use gpui_kit::{div, px, AnyElement, App, Context, Entity, IntoElement, ParentElement, Styled};
use sikemux_core::server::board::{BoardSettingsParams, Landing};
use sikemux_core::server::call;
use sikemux_view::playbook::{Lands, Playbook};

use crate::settings::rows;
use crate::theme::{self, Tokens, SPACE_1H, SPACE_2, TEXT_2XS_H};
use crate::toast;
use crate::ui::{select, Pick};
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
    let about = match lands {
        Lands::Agents => "Agents land their own work once its checks pass",
        Lands::Person => "Agents leave their work for you to land",
    };
    let body = div()
        .child(rows::row_with(
            None,
            "Who lands",
            Some(about.into()),
            who,
            cx,
        ))
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
