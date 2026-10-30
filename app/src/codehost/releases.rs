//! The code host's releases: the list beside the one picked, its notes and its files.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window,
};
use gpui_transcript::{Markdown, Pen};
use serde_json::json;
use sikemux_plugin_api::codehost::{Release, ReleaseList};

use super::{branch_chip, notice, Area, HostArea, Read, Said};
use crate::icons::Icon;
use crate::theme::{
    Tokens, RADIUS_2, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H, SPACE_4,
    SPACE_5, TEXT_3XS, TEXT_3XS_H, TEXT_XS, TEXT_XS_H,
};
use crate::ui::time::ago_written as ago;
use crate::ui::{self, IconButton, Segmented};

pub const KEY: &str = "host-releases";

/// Which releases the list shows.
const KINDS: [&str; 3] = ["All", "Releases", "Pre-releases"];

#[derive(Default)]
pub struct Releases {
    pub list: Read<ReleaseList>,
    kind: usize,
    picked: Option<u64>,
    /// Each release's notes, read as markdown once shown.
    notes: HashMap<u64, Rc<Markdown>>,
    saving: HashSet<u64>,
    assets_folded: bool,
}

impl HostArea {
    fn save_asset(&mut self, id: u64, name: String, cx: &mut Context<Self>) {
        if !self.releases.saving.insert(id) {
            return;
        }
        let saved = self.download("downloadAsset", json!({ "assetId": id, "fileName": name }));
        cx.spawn(async move |area, cx| {
            let saved = saved.await;
            area.update(cx, |area, cx| {
                area.releases.saving.remove(&id);
                let said = match saved {
                    Ok(path) => Said::Done(format!("Saved {name} to {path}").into()),
                    Err(error) => {
                        Said::Failed(format!("Could not download {name}: {error}").into())
                    }
                };
                area.say(said, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

fn shown_in(releases: &Releases, release: &Release) -> bool {
    match releases.kind {
        1 => !(release.prerelease || release.draft),
        2 => release.prerelease || release.draft,
        _ => true,
    }
}

fn tags(release: &Release, latest: bool, cx: &mut Context<HostArea>) -> Vec<AnyElement> {
    let t = Tokens::get(cx).clone();
    let tag = |word: &'static str, ink| {
        div()
            .px(SPACE_1H)
            .rounded_full()
            .border_1()
            .border_color(ink)
            .text_size(crate::theme::ui_text(TEXT_3XS))
            .text_color(ink)
            .child(word)
            .into_any_element()
    };
    let mut tags = Vec::new();
    if latest {
        tags.push(tag("Latest", t.live));
    }
    if release.draft {
        tags.push(tag("Draft", t.text_tertiary));
    }
    if release.prerelease {
        tags.push(tag("Pre-release", t.text_tertiary));
    }
    tags
}

pub fn view(area: &mut HostArea, window: &mut Window, cx: &mut Context<HostArea>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let list = match &area.releases.list {
        None => return notice("Loading releases", None, cx),
        Some(Err(error)) => return notice("Could not read releases", Some(error.clone()), cx),
        Some(Ok(list)) => list,
    };
    let latest = list.latest;
    let shown = list
        .releases
        .iter()
        .find(|release| Some(release.id) == area.releases.picked)
        .or_else(|| {
            list.releases
                .iter()
                .find(|release| Some(release.id) == latest)
        })
        .or_else(|| list.releases.first())
        .cloned();
    if let Some(release) = &shown {
        area.releases
            .notes
            .entry(release.id)
            .or_insert_with(|| Rc::new(crate::editor::preview::render(&release.body)));
    }
    let releases = &area.releases;
    let Some(Ok(list)) = &releases.list else {
        return div().into_any_element();
    };
    let rows = list
        .releases
        .iter()
        .filter(|release| shown_in(releases, release))
        .enumerate()
        .map(|(index, release)| {
            let on = shown.as_ref().is_some_and(|shown| shown.id == release.id);
            let id = release.id;
            let sub = format!(
                "{}{} · {}",
                if release.tag != release.name {
                    format!("{} · ", release.tag)
                } else {
                    String::new()
                },
                release.author.as_deref().unwrap_or("someone"),
                release.published_at.as_deref().map(ago).unwrap_or_default()
            );
            div()
                .id(("release", index))
                .px(SPACE_3)
                .py(SPACE_1H)
                .flex()
                .flex_col()
                .gap(SPACE_0)
                .cursor_pointer()
                .when(on, |row| row.bg(t.surface_chosen))
                .hover(move |row| row.bg(t.surface_hover))
                .on_click(cx.listener(move |area, _, _, cx| {
                    area.releases.picked = Some(id);
                    cx.notify();
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_1H)
                        .child(ui::icon(Icon::Releases, 13.).text_color(t.text_tertiary))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(crate::theme::ui_text(TEXT_XS_H))
                                .text_color(t.text_primary)
                                .child(release.name.clone()),
                        )
                        .children(tags(release, Some(release.id) == latest, cx)),
                )
                .child(ui::meta(sub, cx).pl(px(19.)))
        })
        .collect::<Vec<_>>();
    let side = div()
        .w(px(320.))
        .flex_none()
        .h_full()
        .flex()
        .flex_col()
        .border_r_1()
        .border_color(t.border)
        .child(
            div()
                .px(SPACE_3)
                .pt(SPACE_3)
                .pb(SPACE_2)
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .child(ui::icon(Area::Releases.icon(), 14.).text_color(t.text_tertiary))
                .child(ui::text("Releases", 12., FontWeight::MEDIUM).text_color(t.text_primary))
                .child(div().flex_1())
                .child(super::shell::account(area, cx)),
        )
        .child(
            div().px(SPACE_2).pb(SPACE_2).child(
                Segmented::words(
                    "release-kinds",
                    KINDS
                        .iter()
                        .map(|kind| (SharedString::from(*kind), None))
                        .collect(),
                    releases.kind,
                )
                .fill()
                .on_select(cx.listener(|area, index: &usize, _, cx| {
                    area.releases.kind = *index;
                    cx.notify();
                })),
            ),
        )
        .child(
            div()
                .id("release-rows")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .children(rows)
                .when(
                    list.releases
                        .iter()
                        .all(|release| !shown_in(releases, release)),
                    |rows| {
                        rows.child(
                            div()
                                .px(SPACE_3)
                                .py(SPACE_2)
                                .child(ui::meta("No releases here.", cx)),
                        )
                    },
                ),
        );
    let right = match shown {
        None => notice("Nothing released yet.", None, cx),
        Some(release) => {
            let host = area.host.label.as_str();
            let author = release.author.clone().unwrap_or_else(|| "someone".into());
            let url = release.url.clone();
            let pen = Pen::new(Rc::new(crate::editor::preview::reading_linked(
                13.,
                &area.bench,
                cx,
            )));
            let notes = releases.notes.get(&release.id).cloned();
            let folded = releases.assets_folded;
            let assets = (!release.assets.is_empty()).then(|| {
                div()
                    .mt(SPACE_4)
                    .rounded(RADIUS_2)
                    .border_1()
                    .border_color(t.border)
                    .child(
                        div()
                            .id("release-assets")
                            .px(SPACE_3)
                            .py(px(7.))
                            .flex()
                            .items_center()
                            .gap(SPACE_1H)
                            .cursor_pointer()
                            .text_size(crate::theme::ui_text(TEXT_XS))
                            .text_color(t.text_primary)
                            .on_click(cx.listener(|area, _, _, cx| {
                                area.releases.assets_folded = !area.releases.assets_folded;
                                cx.notify();
                            }))
                            .child("Assets")
                            .child(ui::meta(release.assets.len().to_string(), cx)),
                    )
                    .when(!folded, |box_| {
                        box_.children(release.assets.iter().enumerate().map(|(index, asset)| {
                            let saving = releases.saving.contains(&asset.id);
                            let (id, name) = (asset.id, asset.name.clone());
                            div()
                                .id(("asset", index))
                                .h(px(28.))
                                .px(SPACE_3)
                                .flex()
                                .items_center()
                                .gap(SPACE_2)
                                .border_t_1()
                                .border_color(t.border)
                                .text_size(crate::theme::ui_text(TEXT_XS))
                                .cursor_pointer()
                                .hover(move |row| row.bg(t.surface_hover))
                                .tooltip(ui::tooltip(format!("Download {}", asset.name)))
                                .on_click(cx.listener(move |area, _, _, cx| {
                                    area.save_asset(id, name.clone(), cx)
                                }))
                                .child(ui::icon(Icon::File, 13.).text_color(t.text_tertiary))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_color(t.accent)
                                        .child(asset.name.clone()),
                                )
                                .when(asset.downloads > 0, |row| {
                                    row.child(ui::meta(
                                        format!("{} downloads", asset.downloads),
                                        cx,
                                    ))
                                })
                                .child(ui::meta(super::run_actions::bytes(asset.size_bytes), cx))
                                .child(ui::meta(
                                    if saving {
                                        "Saving…".to_owned()
                                    } else {
                                        release.published_at.as_deref().map(ago).unwrap_or_default()
                                    },
                                    cx,
                                ))
                        }))
                    })
            });
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px(SPACE_4)
                        .pt(SPACE_3H)
                        .pb(SPACE_2H)
                        .flex()
                        .gap(SPACE_3)
                        .border_b_1()
                        .border_color(t.border)
                        .child(ui::initials(&author, "", 36., cx))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(SPACE_1)
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(SPACE_2)
                                        .child(
                                            ui::text(
                                                release.name.clone(),
                                                15.,
                                                FontWeight::SEMIBOLD,
                                            )
                                            .text_color(t.text_primary),
                                        )
                                        .children(tags(&release, Some(release.id) == latest, cx))
                                        .child(
                                            IconButton::new(
                                                "release-on-host",
                                                Icon::Globe,
                                                format!("Open on {host}"),
                                            )
                                            .small()
                                            .on_click(move |_, _, cx| cx.open_url(&url)),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(SPACE_1H)
                                        .text_size(crate::theme::ui_text(TEXT_3XS_H))
                                        .text_color(t.text_tertiary)
                                        .child(format!(
                                            "{author} released this {}",
                                            release
                                                .published_at
                                                .as_deref()
                                                .map(ago)
                                                .unwrap_or_default()
                                        ))
                                        .child(branch_chip(&release.tag, t.text_secondary, cx)),
                                ),
                        ),
                )
                .child(
                    div()
                        .id("release-page")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .px(SPACE_5)
                        .py(SPACE_4)
                        .child(match notes.filter(|_| !release.body.trim().is_empty()) {
                            Some(notes) => pen.markdown(
                                ("release-notes", release.id as usize),
                                &notes,
                                window,
                                cx,
                            ),
                            None => ui::meta("This release has no notes.", cx).into_any_element(),
                        })
                        .children(assets),
                )
                .into_any_element()
        }
    };
    div()
        .size_full()
        .flex()
        .child(side)
        .child(div().flex_1().min_w_0().h_full().child(right))
        .into_any_element()
}
