//! Small parts the pane's pages share: headers, cards, chips and inks.

use super::*;

use crate::theme::{SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_5};

/// A page's head: where it is, its title with what sits beside it, and its tools.
pub(super) fn header(
    crumbs: AnyElement,
    title: impl Into<SharedString>,
    aside: Option<AnyElement>,
    tools: AnyElement,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .px(SPACE_5)
        .pt(SPACE_3)
        .pb(SPACE_2H)
        .flex()
        .flex_col()
        .gap(SPACE_1H)
        .border_b_1()
        .border_color(t.border)
        .child(crumbs)
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(SPACE_2H)
                .child(ui::text(title, 17., FontWeight::SEMIBOLD).text_color(t.text_primary))
                .children(aside)
                .child(div().flex_1())
                .child(tools),
        )
        .into_any_element()
}

pub(super) fn crumbs(project: &str, group: Option<&str>, cx: &App) -> AnyElement {
    let mut path = vec![project.to_owned()];
    path.extend(
        group
            .unwrap_or_default()
            .split('/')
            .filter(|part| !part.is_empty())
            .map(str::to_owned),
    );
    ui::meta(path.join(" › "), cx).into_any_element()
}

pub(super) fn refresh(
    id: &'static str,
    method: &'static str,
    cx: &mut Context<PluginPane>,
) -> AnyElement {
    IconButton::new(id, Icon::Refresh, "Refresh")
        .on_click(listen::<Rundeck, ClickEvent>(
            cx,
            move |_, host, _, _, _| host.read_again(Some(method)),
        ))
        .into_any_element()
}

pub(super) fn card(
    title: &'static str,
    note: impl Into<SharedString>,
    body: AnyElement,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .rounded(theme::RADIUS_2)
        .border_1()
        .border_color(t.border)
        .flex()
        .flex_col()
        .child(
            div()
                .px(SPACE_3)
                .py(SPACE_2)
                .flex()
                .items_center()
                .gap(SPACE_2)
                .border_b_1()
                .border_color(t.line_soft)
                .child(ui::text(title, 12.5, FontWeight::SEMIBOLD).text_color(t.text_primary))
                .child(div().flex_1())
                .child(ui::meta(note.into(), cx).truncate()),
        )
        .child(div().p(SPACE_3).child(body))
        .into_any_element()
}

pub(super) fn banner(text: String, ink: Hsla, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .mx(SPACE_5)
        .my(SPACE_1H)
        .px(SPACE_2H)
        .py(SPACE_1H)
        .rounded(theme::RADIUS_2)
        .border_1()
        .border_color(ink.opacity(0.4))
        .bg(ink.opacity(0.08))
        .child(
            ui::text(text, 12., FontWeight::NORMAL)
                .text_color(t.text_primary)
                .whitespace_normal(),
        )
        .into_any_element()
}

pub(super) fn tag(text: &str, ink: Hsla, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .px(px(5.))
        .rounded(theme::RADIUS_1)
        .border_1()
        .border_color(t.border)
        .child(ui::text(text.to_owned(), 10.5, FontWeight::MEDIUM).text_color(ink))
        .into_any_element()
}

pub(super) fn meta_chip(icon: Icon, text: String, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .px(SPACE_2)
        .py(px(3.))
        .rounded(theme::RADIUS_2)
        .border_1()
        .border_color(t.border)
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .child(ui::icon(icon, 12.).text_color(t.text_tertiary))
        .child(ui::text(text, 12., FontWeight::NORMAL).text_color(t.text_secondary))
        .into_any_element()
}

/// A job's folder as a path, amber when it deploys to production.
pub(super) fn folder_chip(project: &str, group: Option<&str>, prod: bool, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let path = group
        .filter(|group| !group.is_empty())
        .unwrap_or(project)
        .to_owned();
    tag(&path, if prod { t.warn } else { t.text_secondary }, cx)
}

pub(super) fn branch_chip(branch: Option<&str>, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    match branch.filter(|branch| !branch.is_empty()) {
        None => ui::meta("—", cx).into_any_element(),
        Some(branch) => div()
            .flex()
            .items_center()
            .gap(SPACE_1)
            .min_w_0()
            .child(ui::icon(Icon::Git, 11.).text_color(t.text_tertiary))
            .child(
                ui::text(branch.to_owned(), 12., FontWeight::NORMAL)
                    .text_color(t.accent)
                    .truncate(),
            )
            .into_any_element(),
    }
}

pub(super) fn status_ink(kind: StatusKind, t: &Tokens) -> Hsla {
    match kind {
        StatusKind::Succeeded => t.live,
        StatusKind::Failed => t.danger,
        StatusKind::Running => t.warn,
        StatusKind::Aborted => t.text_tertiary,
        StatusKind::Unknown => t.text_disabled,
    }
}

pub(super) fn step_ink(state: StepState, t: &Tokens) -> Hsla {
    match state {
        StepState::Ok => t.live,
        StepState::Fail => t.danger,
        StepState::Running => t.warn,
        StepState::Skip => t.text_disabled,
        StepState::Pending => t.line_soft,
    }
}

pub(super) fn status_dot(kind: Option<StatusKind>, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let ink = kind.map_or(t.line_soft, |kind| status_ink(kind, t));
    div()
        .size(px(7.))
        .flex_none()
        .rounded_full()
        .bg(ink)
        .into_any_element()
}

pub(super) fn capitalised(word: &str) -> String {
    let mut letters = word.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => String::new(),
    }
}
