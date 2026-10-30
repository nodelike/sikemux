//! The code host's pull requests: the list, with each one's state, branches, checks,
//! reviewers and talk.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled,
};
use sikemux_plugin_api::codehost::{ListedPull, PullList};

use super::{
    branch_chip, comments, list_head, notice, outcome_word, pull_mark, tally_summary, Area,
    HostArea, Read,
};
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, TEXT_2XS, TEXT_3XS_H,
    TEXT_XS,
};
use crate::ui;
use crate::ui::time::ago_written as ago;

/// The key the list is watched under.
pub const KEY: &str = "host-pulls";

/// Which pull requests the list shows.
#[derive(Clone, Copy, Default, PartialEq)]
pub enum Listed {
    #[default]
    Open,
    Closed,
    All,
}

impl Listed {
    pub(super) const EVERY: [Self; 3] = [Self::Open, Self::Closed, Self::All];

    pub fn word(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::All => "all",
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Closed => "Closed",
            Self::All => "All",
        }
    }
}

pub struct Pulls {
    pub state: Listed,
    pub list: Read<PullList>,
    /// The project's branch, and whether the core says it needs a pull request.
    pub branch: Option<(String, bool)>,
}

impl Pulls {
    pub fn new(branch: Option<(String, bool)>) -> Self {
        Self {
            state: Listed::default(),
            list: None,
            branch,
        }
    }
}

pub fn list(area: &HostArea, cx: &mut Context<HostArea>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let pulls = &area.pulls;
    let counts = pulls
        .list
        .as_ref()
        .and_then(|list| list.as_ref().ok())
        .and_then(|list| list.counts.clone());
    let words = Listed::EVERY
        .iter()
        .map(|listed| {
            let count = counts.as_ref().and_then(|counts| match listed {
                Listed::Open => Some(counts.open),
                Listed::Closed => Some(counts.closed),
                Listed::All => None,
            });
            (SharedString::from(listed.label()), count)
        })
        .collect();
    let chosen = Listed::EVERY
        .iter()
        .position(|listed| *listed == pulls.state)
        .unwrap_or_default();
    let head = list_head(Area::Pulls, &area.host, cx)
        .child(
            ui::Segmented::words("pull-state", words, chosen).on_select(cx.listener(
                |area, index: &usize, _, cx| {
                    area.pulls.state = Listed::EVERY[*index];
                    area.pulls.list = None;
                    area.watch();
                    cx.notify();
                },
            )),
        )
        .child(div().flex_1())
        .child(
            ui::IconButton::new("pull-new", Icon::Plus, "New pull request")
                .small()
                .on_click(cx.listener(|area, _, window, cx| area.compose(window, cx))),
        )
        .child(super::shell::account(area, cx));
    let listed = matches!(pulls.list, Some(Ok(_)));
    let offer = pulls
        .branch
        .as_ref()
        .filter(|(_, needs)| *needs && listed)
        .map(|(branch, _)| {
            div()
                .mx(SPACE_3)
                .mb(SPACE_2)
                .px(SPACE_3)
                .py(SPACE_2)
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .rounded(theme::RADIUS_2)
                .border_1()
                .border_color(t.border)
                .text_size(crate::theme::ui_text(TEXT_XS))
                .text_color(t.text_secondary)
                .child(branch_chip(branch, t.text_primary, cx))
                .child("has no pull request yet.")
                .child(div().flex_1())
                .child(
                    ui::Button::new("pull-offer", "Open one", ui::Variant::Primary)
                        .on_click(cx.listener(|area, _, window, cx| area.compose(window, cx))),
                )
        });
    let body = match &pulls.list {
        None => notice("Loading pull requests", None, cx),
        Some(Err(error)) => notice("Could not read pull requests", Some(error.clone()), cx),
        Some(Ok(list)) if list.pulls.is_empty() => {
            let which = match pulls.state {
                Listed::All => String::new(),
                listed => format!("{} ", listed.word()),
            };
            notice(format!("No {which}pull requests."), None, cx)
        }
        Some(Ok(list)) => super::carded(div().flex().flex_col(), cx)
            .children(
                list.pulls
                    .iter()
                    .enumerate()
                    .map(|(index, listed)| row(area, listed, index, cx)),
            )
            .into_any_element(),
    };
    let shown = div().flex().flex_col().children(offer).child(body);
    super::list_page("host-list", head, shown, cx)
}

/// One pull request: who opened it, its state and title, then its number, age,
/// branches and checks, with its reviewers, the issues it closes and its comments at
/// the end.
fn row(
    area: &HostArea,
    listed: &ListedPull,
    index: usize,
    cx: &mut Context<HostArea>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let pull = &listed.pull;
    let author = pull.author.as_deref().unwrap_or("someone");
    let (opened, updated) = (ago(&pull.created_at), ago(&pull.updated_at));
    let mut who = format!("#{} · {author} opened {opened}", pull.number);
    if updated != opened {
        who.push_str(&format!(" · updated {updated}"));
    }
    let refs = pull.head.as_ref().map(|head| {
        div()
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .min_w_0()
            .child(branch_chip(head, t.text_secondary, cx))
            .when_some(pull.base.as_ref(), |refs, base| {
                refs.child("→")
                    .child(branch_chip(base, t.text_secondary, cx))
            })
    });
    let checks = listed.checks.as_ref().map(|tally| {
        let summary = tally_summary(tally);
        let about = if summary == "all passed" {
            "All checks passed".to_owned()
        } else {
            format!("Checks: {summary}")
        };
        div()
            .id(("pull-checks", index))
            .flex()
            .flex_none()
            .items_center()
            .gap(SPACE_1H)
            .tooltip(ui::tooltip(about))
            .child("·")
            .child(ui::run_mark(
                format!("pull-checks-{index}"),
                outcome_word(tally.outcome),
                13.,
                cx,
            ))
            .child(
                div()
                    .font_features(theme::tabular())
                    .child(format!("{}/{}", tally.passed, tally.total)),
            )
    });
    let reviewers = (!pull.reviewers.is_empty()).then(|| {
        div()
            .id(("pull-reviewers", index))
            .flex()
            .gap(SPACE_0)
            .tooltip(ui::tooltip(format!(
                "Reviewers: {}",
                pull.reviewers.join(", ")
            )))
            .children(pull.reviewers.iter().map(|login| {
                area.face(
                    Some(login),
                    pull.avatars.get(login).map(String::as_str),
                    18.,
                    cx,
                )
            }))
    });
    let closes = (!listed.closes.is_empty()).then(|| {
        let numbers: Vec<String> = listed
            .closes
            .iter()
            .map(|number| format!("#{number}"))
            .collect();
        div()
            .id(("pull-closes", index))
            .flex()
            .items_center()
            .gap(SPACE_1)
            .text_size(crate::theme::ui_text(TEXT_2XS))
            .text_color(t.text_tertiary)
            .tooltip(ui::tooltip(format!("Closes {}", numbers.join(", "))))
            .child(ui::icon(Icon::Issues, 13.).text_color(t.text_tertiary))
            .child(listed.closes.len().to_string())
    });
    div()
        .id(("pull", index))
        .w_full()
        .px(SPACE_3)
        .py(SPACE_2H)
        .flex()
        .items_center()
        .gap(SPACE_3)
        .when(index > 0, |row| row.border_t_1().border_color(t.border))
        .cursor_pointer()
        .hover(move |row| row.bg(t.surface_hover))
        .on_click({
            let number = pull.number;
            cx.listener(move |area, _, _, cx| area.open_pull(number, cx))
        })
        .child(area.face(Some(author), pull.avatar_url.as_deref(), 28., cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_2)
                        .min_w_0()
                        .child(pull_mark(listed.state, 14., cx))
                        .child(
                            ui::text(pull.title.clone(), 13., FontWeight::SEMIBOLD)
                                .text_color(t.text_primary)
                                .min_w_0()
                                .truncate(),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_1H)
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_size(crate::theme::ui_text(TEXT_3XS_H))
                        .text_color(t.text_tertiary)
                        .child(div().flex_none().child(who))
                        .children(refs)
                        .children(checks),
                ),
        )
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(SPACE_3)
                .children(reviewers)
                .children(closes)
                .children(comments(pull.comments.unwrap_or_default(), cx)),
        )
        .into_any_element()
}
