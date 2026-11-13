//! A quick look at the chosen host at the top of the top bar's host picker: its state, how
//! busy it is and the apps using it most.

use gpui_kit::{
    div, px, Context, FontWeight, IntoElement, ParentElement, Render, Styled, WeakEntity, Window,
};
use sikemux_view::host_stats::ProcessUse;

use super::board::{meter, name_and_model, tile, METERS};
use super::detail::app_icon;
use super::model;
use crate::theme::{Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, TEXT_SM, TEXT_XS};
use crate::ui;
use crate::workbench::Workbench;

const APPS_SHOWN: usize = 3;

pub struct Glance {
    pub bench: WeakEntity<Workbench>,
    /// The host's name and state, else how busy it is and its top apps.
    pub head: bool,
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
        apps.sort_by_key(|app| std::cmp::Reverse(app.cpu));
        apps.truncate(APPS_SHOWN);
        let frame = div().w(px(264.)).py(SPACE_1).flex().flex_col();
        if self.head {
            return frame.child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_2H)
                    .child(tile(host, 30., 15., &t))
                    .child(name_and_model(host, TEXT_SM, &t))
                    .child(
                        div()
                            .ml_auto()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(SPACE_1H)
                            .child(
                                div()
                                    .size(px(6.))
                                    .rounded(crate::theme::radius_round())
                                    .bg(tint),
                            )
                            .child(
                                ui::text(host.state.words(), TEXT_XS, FontWeight::NORMAL)
                                    .text_color(t.text_tertiary),
                            ),
                    ),
            );
        }
        frame
            .children(host.online().then(|| {
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_3)
                    .children(METERS.iter().map(|kind| meter(host, *kind, false, &t, cx)))
            }))
            .children((!apps.is_empty()).then(|| {
                div()
                    .mt(SPACE_3)
                    .flex()
                    .flex_col()
                    .child(div().mb(SPACE_1).child(ui::caps(t!("Top apps"), cx)))
                    .children(apps.into_iter().map(|app| {
                        div()
                            .h(px(22.))
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
