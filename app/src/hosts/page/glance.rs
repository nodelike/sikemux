//! A quick look at the chosen host at the top of the top bar's host picker: its state, how
//! busy it is and the apps using it most.

use gpui_kit::{
    div, px, Context, FontWeight, IntoElement, ParentElement, Render, Styled, WeakEntity, Window,
};
use sikemux_view::host_stats::ProcessUse;

use super::board::{meter, METERS};
use super::detail::app_icon;
use super::model;
use crate::theme::{Tokens, SPACE_1, SPACE_2, SPACE_2H, SPACE_3, TEXT_SM, TEXT_XS};
use crate::ui;
use crate::workbench::Workbench;

const APPS_SHOWN: usize = 3;

pub struct Glance {
    pub bench: WeakEntity<Workbench>,
}

impl Render for Glance {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let Some(bench) = self.bench.upgrade() else {
            return div();
        };
        let bench = bench.read(cx);
        let place = bench.place();
        let listed = model::hosts(bench);
        let Some(host) = listed.iter().find(|host| host.place == place) else {
            return div();
        };
        let tint = host.state.tint(&t);
        let mut apps: Vec<&ProcessUse> = host
            .stats()
            .map_or_else(Vec::new, |stats| stats.top.iter().collect());
        apps.sort_by(|a, b| b.cpu.cmp(&a.cpu));
        apps.truncate(APPS_SHOWN);
        div()
            .w(px(264.))
            .px(SPACE_1)
            .pt(SPACE_1)
            .pb(SPACE_2)
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .child(
                        ui::text(host.name.clone(), TEXT_SM, FontWeight::SEMIBOLD)
                            .truncate()
                            .text_color(t.text_primary),
                    )
                    .child(div().size(px(6.)).flex_none().rounded_full().bg(tint))
                    .child(
                        ui::text(host.state.words(), TEXT_XS, FontWeight::NORMAL)
                            .truncate()
                            .text_color(t.text_tertiary),
                    ),
            )
            .children(host.online().then(|| {
                div()
                    .mt(SPACE_3)
                    .flex()
                    .flex_col()
                    .gap(SPACE_2H)
                    .children(METERS.iter().map(|kind| meter(host, *kind, false, &t, cx)))
            }))
            .children((!apps.is_empty()).then(|| {
                div()
                    .mt(SPACE_3)
                    .flex()
                    .flex_col()
                    .gap(SPACE_1)
                    .child(ui::caps("Top apps", cx))
                    .children(apps.into_iter().map(|app| {
                        div()
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .gap(SPACE_2)
                            .child(app_icon(bench, app, &t))
                            .child(
                                ui::text(app.name.clone(), TEXT_XS, FontWeight::NORMAL)
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(t.text_secondary),
                            )
                            .child(
                                ui::mono(model::percent(app.cpu), TEXT_XS, cx)
                                    .text_color(t.text_tertiary),
                            )
                    }))
            }))
    }
}
