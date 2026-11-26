//! Inline per-tab prompt for a page's `alert`, `confirm`, or `prompt`, scoped to that tab's pane.

use std::collections::HashSet;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, AppContext, Context, Entity, FontWeight, IntoElement, ParentElement,
    Styled, Subscription, Window,
};
use sikemux_browser::BrowserTab;

use crate::theme::{
    border, radius_2, ui_text, Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_3, TEXT_XS,
};
use crate::ui;
use crate::workbench::Workbench;

pub(crate) struct PromptField {
    pub(crate) input: Entity<InputState>,
    _enter: Subscription,
}

/// Keeps a text input per open `prompt` dialog and drops inputs once dialogs close.
pub(super) fn sync(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    ui::forget_spots(|| "page-dialog".into());
    let wanted: Vec<(String, String, String)> = bench
        .desks
        .read(cx)
        .strips
        .iter()
        .flat_map(|(agent, strip)| {
            strip.tabs.iter().filter_map(move |tab| {
                let dialog = tab
                    .dialog
                    .as_ref()
                    .filter(|dialog| dialog.kind == "prompt")?;
                Some((
                    agent.clone(),
                    tab.id.clone(),
                    dialog.default_text.clone().unwrap_or_default(),
                ))
            })
        })
        .collect();

    let live: HashSet<&str> = wanted.iter().map(|(_, tab, _)| tab.as_str()).collect();
    bench.desks.update(cx, |desks, _| {
        desks
            .dialog_prompts
            .retain(|tab, _| live.contains(tab.as_str()));
    });

    for (agent, tab_id, default_text) in wanted {
        if bench.desks.read(cx).dialog_prompts.contains_key(&tab_id) {
            continue;
        }
        let input = cx.new(|cx| InputState::new(window, cx).default_value(default_text));
        let (submit_agent, submit_tab) = (agent.clone(), tab_id.clone());
        let enter = cx.subscribe_in(
            &input,
            window,
            move |bench, _, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    bench.answer_page_dialog(&submit_agent, &submit_tab, true, cx);
                }
            },
        );
        bench.desks.update(cx, |desks, _| {
            desks.dialog_prompts.insert(
                tab_id,
                PromptField {
                    input,
                    _enter: enter,
                },
            );
        });
    }
}

impl Workbench {
    pub(super) fn answer_page_dialog(
        &mut self,
        agent: &str,
        tab_id: &str,
        accept: bool,
        cx: &mut Context<Self>,
    ) {
        let desks = self.desks.read(cx);
        let Some(engine) = desks.engine.clone() else {
            return;
        };
        let text = accept
            .then(|| {
                desks
                    .dialog_prompts
                    .get(tab_id)
                    .map(|field| field.input.read(cx).value().to_string())
            })
            .flatten();
        let (agent, tab_id) = (agent.to_owned(), tab_id.to_owned());
        super::spawn(async move {
            let _ = engine.answer_dialog(&agent, &tab_id, accept, text).await;
        });
    }
}

pub(super) fn banner(
    bench: &Workbench,
    agent: &str,
    front: Option<&BrowserTab>,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let tab = front?;
    let dialog = tab.dialog.as_ref()?;
    let t = Tokens::get(cx).clone();
    let host = url::Url::parse(&tab.url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .filter(|host| !host.is_empty());
    let heading = match host {
        Some(host) => format!("{host} says"),
        None => "This page says".to_owned(),
    };
    let prompt = (dialog.kind == "prompt")
        .then(|| {
            bench
                .desks
                .read(cx)
                .dialog_prompts
                .get(&tab.id)
                .map(|field| field.input.clone())
        })
        .flatten();
    let show_cancel = dialog.kind != "alert";
    let (ok_agent, ok_tab) = (agent.to_owned(), tab.id.clone());
    let (cancel_agent, cancel_tab) = (agent.to_owned(), tab.id.clone());

    Some(
        div()
            .w_full()
            .flex_none()
            .px(SPACE_3)
            .py(SPACE_2)
            .bg(t.surface_raised)
            .border_b(border())
            .border_color(t.border)
            .flex()
            .flex_col()
            .gap(SPACE_1H)
            .children(ui::spot(|| "page-dialog".into()))
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(SPACE_3)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(SPACE_1)
                            .child(
                                div()
                                    .text_size(ui_text(TEXT_XS))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(t.text_tertiary)
                                    .child(heading),
                            )
                            .when(!dialog.message.is_empty(), |col| {
                                col.child(
                                    div()
                                        .text_size(ui_text(TEXT_XS))
                                        .text_color(t.text_primary)
                                        .child(dialog.message.clone()),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(SPACE_1H)
                            .when(show_cancel, |actions| {
                                actions.child(
                                    ui::Button::new(
                                        "page-dialog-cancel",
                                        t!("Cancel"),
                                        ui::Variant::Outline,
                                    )
                                    .on_click(cx.listener(
                                        move |bench, _, _, cx| {
                                            bench.answer_page_dialog(
                                                &cancel_agent,
                                                &cancel_tab,
                                                false,
                                                cx,
                                            );
                                        },
                                    )),
                                )
                            })
                            .child(
                                ui::Button::new("page-dialog-ok", "OK", ui::Variant::Primary)
                                    .on_click(cx.listener(move |bench, _, _, cx| {
                                        bench.answer_page_dialog(&ok_agent, &ok_tab, true, cx);
                                    })),
                            ),
                    ),
            )
            .children(prompt.map(|input| {
                div()
                    .h(px(28.))
                    .px(SPACE_2)
                    .flex()
                    .items_center()
                    .rounded(radius_2())
                    .border(border())
                    .border_color(t.border)
                    .bg(t.surface_sunken)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&input).appearance(false)),
                    )
            }))
            .into_any_element(),
    )
}
