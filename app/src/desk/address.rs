//! ⌘L's address: a field floating over the middle of a desk's page, which finishes a
//! remembered site as it is typed and lists the pages that match below it.

use std::ops::Range;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    actions, deferred, div, px, relative, AnyElement, App, AppContext, Context, Entity, FontWeight,
    HighlightStyle, InteractiveElement, IntoElement, KeyBinding, MouseButton, ParentElement,
    StatefulInteractiveElement, Styled, StyledText, Subscription, Task, Window,
};
use sikemux_browser::history::{AddressSuggestions, Suggestion};

use crate::icons::Icon;
use crate::theme::{
    ui_text, Tokens, RADIUS_2, SPACE_0, SPACE_2, SPACE_2H, TEXT_SM, TEXT_XL, TEXT_XS,
};
use crate::ui;
use crate::workbench::Workbench;

actions!(address, [Next, Previous, Go, Close]);

const MAX_WIDTH: f32 = 680.;
const MARGIN: f32 = 16.;
/// How far down the page the field sits, unless a long list needs it higher.
const FIELD_AT: f32 = 0.36;
/// How dark the page goes under the address.
const UNDER_DIM: f32 = 0.2;

pub fn install(cx: &mut App) {
    let context = Some("FloatingAddress > Input");
    cx.bind_keys([
        KeyBinding::new("down", Next, context),
        KeyBinding::new("up", Previous, context),
        KeyBinding::new("enter", Go, context),
        KeyBinding::new("escape", Close, context),
    ]);
}

/// The open address and what it offers.
pub(super) struct Floating {
    pub(super) agent: String,
    pub(super) input: Entity<InputState>,
    /// What the person typed, without the part finished for them.
    typed: String,
    /// Only typing forward is finished; deleting the finished part must not bring it back.
    completing: bool,
    found: Option<AddressSuggestions>,
    /// None while the empty field offers the most visited sites, until an arrow picks one.
    selected: Option<usize>,
    _typing: Subscription,
    _asking: Option<Task<()>>,
}

struct Suggested {
    to: Target,
    title: String,
    detail: String,
    icon: Option<String>,
    search: bool,
}

enum Target {
    Page(String),
    Typed,
}

impl Floating {
    fn idle(&self) -> bool {
        self.typed.trim().is_empty()
    }

    fn suffix(&self) -> &str {
        let Some(done) = (self.found.as_ref())
            .and_then(|found| found.completion.as_ref())
            .filter(|_| self.completing)
        else {
            return "";
        };
        match done.address.get(..self.typed.len()) {
            Some(head)
                if done.address.len() > self.typed.len()
                    && head.eq_ignore_ascii_case(&self.typed) =>
            {
                &done.address[self.typed.len()..]
            }
            _ => "",
        }
    }

    /// The rows under the field; `shown` is the page open now, which the top sites leave out.
    fn rows(&self, shown: &str) -> Vec<Suggested> {
        let Some(found) = &self.found else {
            return Vec::new();
        };
        let page = |page: &Suggestion| Suggested {
            to: Target::Page(page.url.clone()),
            title: match page.title.is_empty() {
                true => page.address.clone(),
                false => page.title.clone(),
            },
            detail: page.address.clone(),
            icon: page.icon.clone(),
            search: false,
        };
        if self.idle() {
            return (found.pages.iter())
                .filter(|page| page.url != shown)
                .map(page)
                .collect();
        }
        let search = || Suggested {
            to: Target::Typed,
            title: self.typed.clone(),
            detail: "Google Search".into(),
            icon: None,
            search: true,
        };
        let suffix = self.suffix();
        let mut rows = vec![match &found.completion {
            Some(done) if !suffix.is_empty() => Suggested {
                to: Target::Page(done.url.clone()),
                title: done.title.clone(),
                detail: format!("{}{suffix}", self.typed),
                icon: done.icon.clone(),
                search: false,
            },
            _ if found.searches => search(),
            _ => Suggested {
                to: Target::Typed,
                title: self.typed.clone(),
                detail: String::new(),
                icon: None,
                search: false,
            },
        }];
        rows.extend(found.pages.iter().map(page));
        if !rows[0].search && found.searches {
            rows.push(search());
        }
        rows
    }

    fn listed(&self, rows: &[Suggested]) -> bool {
        match self.idle() {
            true => !rows.is_empty(),
            false => rows.len() > 1 || !self.suffix().is_empty(),
        }
    }

    /// Writes the remembered address's rest after what was typed, selected to type over.
    fn finish(&self, window: &mut Window, cx: &mut App) {
        let suffix = self.suffix();
        if suffix.is_empty() || self.input.read(cx).value() != self.typed.as_str() {
            return;
        }
        let whole = format!("{}{suffix}", self.typed);
        let from = self.typed.len();
        self.input.update(cx, |input, cx| {
            input.set_value(whole.clone(), window, cx);
            input.set_selected_range(from..whole.len(), cx);
        });
    }
}

impl Workbench {
    /// Floats the agent's address over its page, holding the page's address all selected.
    pub(super) fn float_address(
        &mut self,
        agent: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let shown = super::shown_address(self.front_url(&agent, cx).unwrap_or_default());
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search or enter address")
                .default_value(shown)
        });
        let typing = cx.subscribe_in(
            &input,
            window,
            |bench, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => bench.address_typed(window, cx),
                InputEvent::Blur => bench.close_address(false, window, cx),
                _ => {}
            },
        );
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        let floating = Floating {
            agent,
            input,
            typed: String::new(),
            completing: false,
            found: None,
            selected: None,
            _typing: typing,
            _asking: None,
        };
        self.desks
            .update(cx, |desks, _| desks.floating = Some(floating));
        self.ask_address(window, cx);
    }

    fn address_rows(&self, cx: &App) -> Vec<Suggested> {
        let Some(floating) = &self.desks.read(cx).floating else {
            return Vec::new();
        };
        floating.rows(self.front_url(&floating.agent, cx).unwrap_or_default())
    }

    fn address_typed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.desks.update(cx, |desks, cx| {
            let floating = desks.floating.as_mut()?;
            let input = floating.input.read(cx);
            let value = input.value().to_string();
            floating.completing =
                input.cursor() == value.len() && value.len() > floating.typed.len();
            floating.typed = value;
            floating.selected = (!floating.idle()).then_some(0);
            floating.finish(window, cx);
            cx.notify();
            Some(())
        });
        if typed.is_some() {
            self.ask_address(window, cx);
        }
    }

    /// Asks the browser's history what to offer for what is typed, off the window's thread.
    fn ask_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let desks = self.desks.read(cx);
        let (Some(engine), Some(query)) = (
            desks.engine.clone(),
            desks
                .floating
                .as_ref()
                .map(|floating| floating.typed.clone()),
        ) else {
            return;
        };
        let asking = cx.spawn_in(window, async move |bench, cx| {
            let asked = query.clone();
            let Some(found) = super::run(async move { engine.suggest(&asked) }).await else {
                return;
            };
            let _ = bench.update_in(cx, |bench, window, cx| {
                bench.desks.update(cx, |desks, cx| {
                    let Some(floating) =
                        (desks.floating.as_mut()).filter(|floating| floating.typed == query)
                    else {
                        return;
                    };
                    floating.found = Some(found);
                    floating.finish(window, cx);
                    cx.notify();
                });
            });
        });
        self.desks.update(cx, |desks, _| {
            if let Some(floating) = desks.floating.as_mut() {
                floating._asking = Some(asking);
            }
        });
    }

    fn step_address(&mut self, by: isize, cx: &mut Context<Self>) {
        let rows = self.address_rows(cx);
        self.desks.update(cx, |desks, cx| {
            let Some(floating) = desks.floating.as_mut() else {
                return;
            };
            if !floating.listed(&rows) {
                return;
            }
            let first = if floating.idle() { -1 } else { 0 };
            let at = floating.selected.map_or(-1, |at| at as isize);
            let next = (at + by).clamp(first, rows.len() as isize - 1);
            floating.selected = usize::try_from(next).ok();
            cx.notify();
        });
    }

    /// Opens the row picked, else what the field holds, and hands the page the keys.
    fn go_address(&mut self, picked: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.address_rows(cx);
        let Some(floating) = self.desks.read(cx).floating.as_ref() else {
            return;
        };
        let agent = floating.agent.clone();
        let value = floating.input.read(cx).value().trim().to_string();
        let picked = picked
            .or(floating.selected)
            .filter(|_| floating.listed(&rows));
        match picked.and_then(|at| rows.into_iter().nth(at)) {
            Some(Suggested {
                to: Target::Page(url),
                ..
            }) => match self.front_url(&agent, cx) {
                Some(_) => self.with_engine(cx, |engine| engine.navigate(&agent, &url).map(drop)),
                None => {
                    self.show_on_desk(&agent, &url, cx);
                }
            },
            _ => self.go_typed(&agent, &value, cx),
        }
        self.close_address(true, window, cx);
    }

    /// Closes the address, handing the keys to the page under it when `to_page`.
    fn close_address(&mut self, to_page: bool, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.desks.update(cx, |desks, cx| {
            let floating = desks.floating.take()?;
            cx.notify();
            let page = desks.pages.get(&floating.agent).filter(|_| to_page)?;
            Some(page.focus.clone())
        });
        if let Some(focus) = focus {
            focus.focus(window, cx);
        }
    }
}

/// The address floating over `agent`'s page, with the page dimmed under it.
pub(super) fn overlay(
    bench: &Workbench,
    agent: &str,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let floating =
        (bench.desks.read(cx).floating.as_ref()).filter(|floating| floating.agent == agent)?;
    let t = Tokens::get(cx).clone();
    let rows = bench.address_rows(cx);
    let listed = floating.listed(&rows);
    let typed = floating.typed.to_lowercase();
    let words: Vec<&str> = typed.split_whitespace().collect();
    let list =
        div()
            .pt(px(5.))
            .flex()
            .flex_col()
            .gap(SPACE_0)
            .children(rows.iter().enumerate().map(|(at, row)| {
                let selected = floating.selected == Some(at);
                let mark = match (row.search, row.icon.as_deref()) {
                    (true, _) => ui::icon(Icon::Search, 13.).into_any_element(),
                    (false, Some(icon)) => match bench.desks.read(cx).favicon(icon) {
                        Some(icon) => gpui_kit::img(icon).size(px(13.)).into_any_element(),
                        None => ui::icon(Icon::Globe, 13.).into_any_element(),
                    },
                    (false, None) => ui::icon(Icon::Globe, 13.).into_any_element(),
                };
                div()
                    .id(("address-row", at))
                    .h(px(32.))
                    .px(SPACE_2)
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .rounded(RADIUS_2)
                    .text_size(ui_text(TEXT_XS))
                    .text_color(t.text_primary)
                    .map(|line| match selected {
                        true => line.bg(t.surface_chosen),
                        false => line.hover(|line| line.bg(t.surface_hover)),
                    })
                    .on_click(cx.listener(move |bench, _, window, cx| {
                        bench.go_address(Some(at), window, cx)
                    }))
                    .child(
                        div()
                            .w(px(14.))
                            .flex_none()
                            .flex()
                            .justify_center()
                            .text_color(t.text_tertiary)
                            .child(mark),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(line_text(row, &words, cx)),
                    )
            }));
    let panel = ui::float(cx)
        .key_context("FloatingAddress")
        .w_full()
        .max_w(px(MAX_WIDTH))
        .flex_none()
        .flex()
        .flex_col()
        .p(px(ui::FLOAT_INSET))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_action(cx.listener(|bench, _: &Next, _, cx| bench.step_address(1, cx)))
        .on_action(cx.listener(|bench, _: &Previous, _, cx| bench.step_address(-1, cx)))
        .on_action(cx.listener(|bench, _: &Go, window, cx| bench.go_address(None, window, cx)))
        .on_action(
            cx.listener(|bench, _: &Close, window, cx| bench.close_address(true, window, cx)),
        )
        .child(
            div()
                .h(px(38.))
                .pl(SPACE_2)
                .flex()
                .items_center()
                .child(ui::icon(Icon::Search, 14.).text_color(t.text_tertiary))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px(SPACE_2H)
                        .h(ui_text(TEXT_XL))
                        .flex()
                        .items_center()
                        .child(
                            Input::new(&floating.input)
                                .appearance(false)
                                .flex_1()
                                .p_0()
                                .text_size(ui_text(TEXT_SM))
                                .text_color(t.text_primary),
                        ),
                ),
        )
        .when(listed, |panel| {
            panel
                .child(div().mx(SPACE_2).h(px(1.)).bg(t.border))
                .child(list)
        });
    Some(
        deferred(
            div()
                .id("address-float")
                .absolute()
                .inset_0()
                .p(px(MARGIN))
                .flex()
                .flex_col()
                .items_center()
                .bg(gpui_kit::black().opacity(UNDER_DIM))
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|bench, _, window, cx| bench.close_address(true, window, cx)),
                )
                .child(div().w_full().h(relative(FIELD_AT)).min_h_0())
                .child(panel),
        )
        .with_priority(1)
        .into_any_element(),
    )
}

/// A row's title, then its address faint after a dash, each typed word in bold where it shows.
fn line_text(row: &Suggested, words: &[&str], cx: &App) -> StyledText {
    let t = Tokens::get(cx);
    let bold = |weight| HighlightStyle {
        font_weight: Some(weight),
        ..Default::default()
    };
    let mut text = row.title.clone();
    let mut runs = marked(
        &row.title,
        words,
        HighlightStyle::default(),
        bold(FontWeight::SEMIBOLD),
        0,
    );
    if !row.detail.is_empty() {
        text.push_str(" — ");
        let faint = HighlightStyle {
            color: Some(t.text_tertiary),
            ..Default::default()
        };
        runs.push((row.title.len()..text.len(), faint));
        let words = if row.search { &[][..] } else { words };
        let found = HighlightStyle {
            color: Some(t.text_secondary),
            ..bold(FontWeight::MEDIUM)
        };
        runs.extend(marked(&row.detail, words, faint, found, text.len()));
        text.push_str(&row.detail);
    }
    StyledText::new(text).with_highlights(runs)
}

/// `text` cut into runs of `plain` and `found`, `found` where a word of `words` shows, from `at`.
fn marked(
    text: &str,
    words: &[&str],
    plain: HighlightStyle,
    found: HighlightStyle,
    at: usize,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut hit = vec![false; text.len()];
    for word in words
        .iter()
        .filter(|word| !word.is_empty() && word.len() <= text.len())
    {
        for start in (0..=text.len() - word.len()).filter(|&start| {
            text.is_char_boundary(start)
                && text.as_bytes()[start..start + word.len()].eq_ignore_ascii_case(word.as_bytes())
        }) {
            hit[start..start + word.len()].fill(true);
        }
    }
    let mut runs: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    for (index, &on) in hit.iter().enumerate() {
        let style = if on { found } else { plain };
        match runs.last_mut() {
            Some((range, last)) if *last == style => range.end = at + index + 1,
            _ => runs.push((at + index..at + index + 1, style)),
        }
    }
    runs
}
