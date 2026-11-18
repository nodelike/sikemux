//! The hosts in the top bar: the Devices button, which opens the chosen host's glance and the
//! hosts to switch to, and the offer to set up an SSH host while one applies.

use gpui_kit::base::Button as BaseButton;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, Anchor, AnyElement, AppContext, Context, FontWeight, Hsla, InteractiveElement,
    IntoElement, ParentElement, Styled, WeakEntity,
};

use crate::hosts::install_pane::Words;
use crate::hosts::page::glance::Glance;
use crate::hosts::HostChoice;
use crate::icons::Icon;
use crate::keymap;
use crate::theme::{Tokens, SPACE_1H, SPACE_2H, TEXT_XS};
use crate::ui;
use crate::workbench::actions;
use crate::workbench::Workbench;
use sikemux_calls::hosts::Unanswered as HostUnanswered;

/// The Devices button: the chosen host's glance, the hosts to switch to, then the Devices page.
pub fn devices(cx: &mut Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let bench = cx.entity().downgrade();
    let trigger = crate::tabs::glyph_chip(
        "devices",
        Icon::Gauge,
        match keymap::hint(actions::FocusHosts::ID, cx) {
            Some(keys) => t!("Devices — {keys}", keys = keys),
            None => t!("Devices").to_owned(),
        },
    );
    ui::menu_button(
        "devices-menu",
        Anchor::TopRight,
        trigger,
        move |popup, _, cx| {
            let Ok((chosen, hosts)) = bench.update(cx, |bench, _| {
                let hosts: Vec<_> = bench.host_choices();
                (bench.chosen_host(), hosts)
            }) else {
                return popup;
            };
            let mut popup = glance(popup.max_h(px(560.)).scrollable(true), bench.clone(), cx);
            for host in hosts {
                let (glyph, choice, picking) =
                    (host_mark(&host.choice, &t), host.choice, bench.clone());
                popup = popup.item(
                    ui::MenuRow::new(host.label)
                        .mark(move || mark(glyph))
                        .item(cx)
                        .checked(choice == chosen)
                        .on_click(move |_, _, cx| {
                            let choice = choice.clone();
                            picking
                                .update(cx, |bench, cx| bench.choose_host(choice, cx))
                                .ok();
                        }),
                );
            }
            popup.separator().item(
                ui::MenuRow::new(t!("Open Devices"))
                    .mark(move || mark((Icon::Gauge, t.text_secondary)))
                    .when_some(keymap::hint(actions::FocusHosts::ID, cx), |row, hint| {
                        row.hint(hint)
                    })
                    .item(cx)
                    .on_click(|_, window, cx| {
                        keymap::dispatch(actions::FocusHosts::ID, window, cx)
                    }),
            )
        },
    )
    .into_any_element()
}

fn host_mark(choice: &HostChoice, t: &Tokens) -> (Icon, Hsla) {
    match choice {
        HostChoice::This => (Icon::Laptop, t.text_primary),
        HostChoice::Account(_) => (Icon::Laptop, t.host),
        HostChoice::Ssh(_) => (Icon::Laptop, t.ssh),
    }
}

/// The button that sets Sikemux up on the chosen SSH host as it needs, saying so while it runs
/// (ADR 0019); none while the chosen host needs nothing set up.
pub fn set_up_offer(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let alias = match bench.chosen_host() {
        HostChoice::Ssh(alias) => alias,
        HostChoice::Account(key) => bench
            .reads
            .hosts
            .iter()
            .find(|host| host.id.as_ref() == Some(&key))
            .and_then(|host| host.ssh.clone())
            .filter(|alias| {
                bench.set_up_of(alias).is_some()
                    || bench.setting_up(alias).is_some()
                    || bench.set_up_problem(alias).is_some()
            })?,
        HostChoice::This => return None,
    };
    let t = Tokens::get(cx).clone();
    let running = bench.setting_up(&alias);
    let problem = bench.set_up_problem(&alias);
    let checking = bench
        .ssh_row(&alias)
        .and_then(|row| row.unanswered.as_ref())
        .is_some_and(|unanswered| *unanswered == HostUnanswered::Checking);
    let offered = bench.set_up_offered(&alias);
    let (mark_of, set_up, label) = match (running, problem, offered) {
        (Some(running), _, _) => (running, None, running.running(&alias)),
        (None, Some((work, problem)), offered) => (
            work,
            offered,
            t!(
                "{work} stopped short: {problem}",
                work = work.menu_label(),
                problem = problem
            ),
        ),
        (None, None, Some(set_up)) => (set_up, Some(set_up), set_up.offer(&alias)),
        (None, None, None) if checking => {
            return Some(
                div()
                    .h(px(26.))
                    .px(SPACE_2H)
                    .flex()
                    .items_center()
                    .text_size(crate::theme::ui_text(TEXT_XS))
                    .text_color(t.text_tertiary)
                    .child(t!("Checking {alias}…", alias = alias))
                    .into_any_element(),
            )
        }
        (None, None, None) => return None,
    };
    Some(
        BaseButton::new("set-up-offer")
            .accessibility_label(label.clone())
            .disabled(set_up.is_none())
            .h(px(26.))
            .max_w(px(260.))
            .px(SPACE_2H)
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .rounded_full()
            .text_size(crate::theme::ui_text(TEXT_XS))
            .font_weight(FontWeight::MEDIUM)
            .text_color(if set_up.is_none() {
                t.text_tertiary
            } else {
                t.text_primary
            })
            .hover(move |style| style.bg(t.surface_hover))
            .child(mark((mark_of.icon(), t.text_secondary)))
            .child(div().min_w_0().truncate().child(label))
            .on_click(cx.listener(move |bench, _, _, cx| {
                if let Some(set_up) = set_up {
                    bench.set_up_host(&alias, set_up, cx)
                }
            }))
            .into_any_element(),
    )
}

/// The chosen host's glance above the hosts, its stats watched only while the menu is open.
fn glance(
    popup: PopupMenu,
    bench: WeakEntity<Workbench>,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let part = |head: bool, cx: &mut Context<PopupMenu>| {
        let bench = bench.clone();
        cx.new(|_| Glance { bench, head })
    };
    let (head, body) = (part(true, cx), part(false, cx));
    let shown = vec![head.downgrade(), body.downgrade()];
    bench
        .update(cx, |bench, _| {
            bench.hosts_page.glance = shown;
            bench.watch_hosts_page();
        })
        .ok();
    let closing = bench.clone();
    cx.on_release(move |_, cx| {
        closing
            .update(cx, |bench, _| {
                bench.hosts_page.glance.clear();
                bench.watch_hosts_page();
            })
            .ok();
    })
    .detach();
    popup
        .item(PopupMenuItem::element(move |_, _| head.clone()).disabled(true))
        .separator()
        .item(PopupMenuItem::element(move |_, _| body.clone()).disabled(true))
        .separator()
}

fn mark((icon, color): (Icon, Hsla)) -> impl IntoElement {
    ui::icon(icon, 13.).text_color(color)
}
