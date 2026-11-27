use std::rc::Rc;

use gpui_kit::{
    canvas, div, px, relative, AnyElement, App, ClipboardItem, FocusHandle, FontWeight,
    InteractiveElement, KeyBinding, MouseButton, ParentElement, Pixels, ScrollHandle, SharedString,
    Styled, Window,
};
use gpui_transcript::{Copy, DocSelect, LinkIcon, Markdown, OnLink, Pen, Style};
use sikemux_view::calls::{CallKind, PersonCall};

use super::{context_words, Through};
use crate::theme::{self, Tokens, SPACE_1};
use crate::ui;

pub(super) const CONTEXT: &str = "CallsProse";
pub(super) const POPOVER_CONTEXT: &str = "QuestionDock CallsProse";

pub(crate) fn install(cx: &mut App) {
    let copy = if cfg!(target_os = "macos") {
        "cmd-c"
    } else {
        "ctrl-c"
    };
    cx.bind_keys([KeyBinding::new(copy, Copy, Some(CONTEXT))]);
}

pub(super) struct RenderedCall {
    id: String,
    headline_src: String,
    body_src: Option<String>,
    context_src: Option<String>,
    evidence_src: Vec<String>,
    against_src: Option<String>,
    pub(super) headline: Rc<Markdown>,
    pub(super) body: Option<Rc<Markdown>>,
    pub(super) context: Option<Rc<Markdown>>,
    pub(super) evidence: Vec<Rc<Markdown>>,
    pub(super) against: Option<Rc<Markdown>>,
}

impl RenderedCall {
    pub(super) fn id(&self) -> &str {
        &self.id
    }

    pub(super) fn sync(slot: &mut Option<Rc<Self>>, call: &PersonCall) -> (Rc<Self>, bool) {
        let (headline_src, body_src) = question_parts(call);
        let context_src = context_words(call);
        let evidence_src: Vec<String> = (call.evidence.iter())
            .filter(|evidence| evidence.picture.is_none())
            .map(|evidence| evidence.what.clone())
            .collect();
        let against_src = call.against.clone();
        let unchanged = slot.as_ref().is_some_and(|kept| {
            kept.id == call.id
                && kept.headline_src == headline_src
                && kept.body_src == body_src
                && kept.context_src == context_src
                && kept.evidence_src == evidence_src
                && kept.against_src == against_src
        });
        if !unchanged {
            *slot = None;
        }
        let rendered = slot.get_or_insert_with(|| {
            let render = |text: &str| Rc::new(crate::reading::render(text));
            Rc::new(Self {
                id: call.id.clone(),
                headline: render(&headline_src),
                body: body_src.as_deref().map(render),
                context: context_src.as_deref().map(render),
                evidence: evidence_src.iter().map(|what| render(what)).collect(),
                against: against_src.as_deref().map(render),
                headline_src,
                body_src,
                context_src,
                evidence_src,
                against_src,
            })
        });
        (Rc::clone(rendered), !unchanged)
    }

    pub(super) fn docs(&self) -> Rc<[Rc<Markdown>]> {
        let mut docs = Vec::with_capacity(3 + self.evidence.len());
        docs.push(self.headline.clone());
        docs.extend(self.body.clone());
        docs.extend(self.context.clone());
        docs.extend(self.evidence.iter().cloned());
        docs.extend(self.against.clone());
        docs.into()
    }
}

pub(super) struct ProseStyles {
    pub(super) headline: Rc<Style>,
    pub(super) body: Rc<Style>,
    pub(super) evidence: Rc<Style>,
    pub(super) against: Rc<Style>,
}

impl ProseStyles {
    pub(super) fn page(call: &PersonCall, through: Through, cx: &App) -> Self {
        Self::build(
            call,
            through,
            None,
            (21., 1.3, 8.),
            (13.5, 1.6, 10.),
            (12.5, 1.5, 8.),
            cx,
        )
    }

    pub(super) fn popover(
        call: &PersonCall,
        through: Through,
        dismiss: crate::ui::list_picker::Dismiss,
        cx: &App,
    ) -> Self {
        Self::build(
            call,
            through,
            Some(dismiss),
            (15., 1.35, 6.),
            (12.5, 1.5, 8.),
            (12.5, 1.5, 8.),
            cx,
        )
    }

    fn build(
        call: &PersonCall,
        through: Through,
        dismiss: Option<crate::ui::list_picker::Dismiss>,
        (headline_size, headline_lh, headline_gap): (f32, f32, f32),
        (body_size, body_lh, body_gap): (f32, f32, f32),
        (against_size, against_lh, against_gap): (f32, f32, f32),
        cx: &App,
    ) -> Self {
        let t = Tokens::get(cx);
        let bench = through.bench.clone();
        let place = through.target.place().clone();
        let held = call.clone();
        let on_link: OnLink = Rc::new(move |href: &str, window: &mut Window, cx: &mut App| {
            if let Some(dismiss) = &dismiss {
                dismiss(window, cx);
            }
            let desk = (bench.read_with(cx, |bench, _| bench.desk_showing(&place, &held)))
                .unwrap_or_else(|_| held.desk().to_owned());
            crate::chat::link::open(href, &desk, &bench, window, cx);
        });
        let link_icon: LinkIcon = Rc::new(|href: &str, size, cx: &mut App| {
            let mark = crate::chat::link::service_mark(href)?;
            Some(ui::picture(
                mark,
                f32::from(size),
                Tokens::get(cx).text_primary,
            ))
        });
        let body = Style {
            line_height: body_lh,
            gap: px(body_gap),
            ink: t.text_secondary,
            strong_ink: Some(t.text_primary),
            on_link: Some(on_link.clone()),
            link_icon: Some(link_icon.clone()),
            ..crate::reading::reading_style(body_size, cx)
        };
        let headline = Style {
            line_height: headline_lh,
            gap: px(headline_gap),
            ink: t.text_primary,
            strong_ink: None,
            on_link: Some(on_link.clone()),
            link_icon: Some(link_icon.clone()),
            ..crate::reading::reading_style(headline_size, cx)
        };
        let evidence = Style {
            ink: t.text_tertiary,
            strong_ink: Some(t.text_secondary),
            ..body.clone()
        };
        let against = Style {
            line_height: against_lh,
            gap: px(against_gap),
            ink: t.text_secondary,
            strong_ink: Some(t.text_primary),
            on_link: Some(on_link),
            link_icon: Some(link_icon),
            ..crate::reading::reading_style(against_size, cx)
        };
        Self {
            headline: Rc::new(headline),
            body: Rc::new(body),
            evidence: Rc::new(evidence),
            against: Rc::new(against),
        }
    }
}

pub(super) struct Prose<'a> {
    pub(super) pen: Pen,
    pub(super) rendered: &'a RenderedCall,
    pub(super) styles: ProseStyles,
    pub(super) select: DocSelect,
    pub(super) focus: FocusHandle,
    pub(super) docs: Rc<[Rc<Markdown>]>,
}

impl<'a> Prose<'a> {
    pub(super) fn new(
        rendered: &'a RenderedCall,
        styles: ProseStyles,
        select: DocSelect,
        focus: FocusHandle,
    ) -> Self {
        let docs = rendered.docs();
        let doc_refs: Vec<&Markdown> = docs.iter().map(Rc::as_ref).collect();
        let pen = select.pen_many(styles.body.clone(), &doc_refs, None);
        Self {
            pen,
            rendered,
            styles,
            select,
            focus,
            docs,
        }
    }

    pub(super) fn selectable(&self, area: gpui_kit::Div) -> gpui_kit::Div {
        let (focus, pressed, docs) = (self.focus.clone(), self.select.clone(), self.docs.clone());
        area.on_mouse_down(MouseButton::Left, move |event, window, cx| {
            focus.focus(window, cx);
            let refs: Vec<&Markdown> = docs.iter().map(Rc::as_ref).collect();
            pressed.press_many(event, &refs);
            window.refresh();
        })
    }

    pub(super) fn question(
        &self,
        prefix: &str,
        call_id: &str,
        body_gap: Pixels,
        max_width: Option<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> gpui_kit::Div {
        let headline = div()
            .font_weight(FontWeight::SEMIBOLD)
            .child(self.pen.markdown_with(
                &self.styles.headline,
                SharedString::from(format!("{prefix}-question-{call_id}")),
                &self.rendered.headline,
                window,
                cx,
            ));
        let body = self.rendered.body.as_ref().map(|body| {
            let drawn = self.pen.markdown(
                SharedString::from(format!("{prefix}-body-{call_id}")),
                body,
                window,
                cx,
            );
            block(drawn, max_width).mt(body_gap)
        });
        self.selectable(div().flex().flex_col().child(headline).children(body))
    }

    pub(super) fn against_row(
        &self,
        id: SharedString,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<gpui_kit::Div> {
        let t = Tokens::get(cx);
        let against = self.rendered.against.as_ref()?;
        Some(
            self.selectable(
                div()
                    .flex()
                    .items_start()
                    .gap(SPACE_1)
                    .child(
                        div()
                            .flex_none()
                            .text_size(theme::ui_text(12.5))
                            .line_height(relative(1.5))
                            .text_color(t.text_tertiary)
                            .child(t!("Against it:")),
                    )
                    .child(div().flex_1().min_w_0().child(self.pen.markdown_with(
                        &self.styles.against,
                        id,
                        against,
                        window,
                        cx,
                    ))),
            ),
        )
    }

    pub(super) fn copy_action(&self) -> impl Fn(&Copy, &mut Window, &mut App) + 'static {
        let (copied, copying) = (self.select.clone(), self.docs.clone());
        move |_: &Copy, window, cx| {
            if !gpui_kit::base::TextSelection::selected_text(window, cx).is_empty() {
                cx.propagate();
                return;
            }
            let refs: Vec<&Markdown> = copying.iter().map(Rc::as_ref).collect();
            if let Some(text) = copied.selected_text_many(&refs) {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            } else {
                cx.propagate();
            }
        }
    }

    pub(super) fn drag_overlay(&self, scroll: ScrollHandle) -> gpui_kit::Canvas<()> {
        let followed = self.select.clone();
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| followed.follow(bounds, &scroll, window),
        )
        .absolute()
        .inset_0()
    }
}

pub(super) fn block(drawn: AnyElement, max_width: Option<Pixels>) -> gpui_kit::Div {
    let mut box_ = div().min_w_0();
    if let Some(max_width) = max_width {
        box_ = box_.max_w(max_width);
    }
    box_.child(drawn)
}

fn trim_blank_lines(text: &str) -> &str {
    let mut start = 0;
    let mut rest = text;
    while let Some(newline) = rest.find('\n') {
        let line = rest.get(..newline).unwrap_or_default();
        if !line.trim().is_empty() {
            break;
        }
        start += newline + 1;
        rest = rest.get(newline + 1..).unwrap_or_default();
    }
    text.get(start..).unwrap_or_default().trim_end()
}

fn strip_heading_prefix(line: &str) -> &str {
    let hashes = line.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&hashes) {
        return line;
    }
    match line.get(hashes..) {
        Some("") => "",
        Some(rest) if rest.starts_with([' ', '\t']) => rest.trim_start(),
        _ => line,
    }
}

/// The headline that says what `call` asks, and the markdown body under it.
pub(super) fn question_parts(call: &PersonCall) -> (String, Option<String>) {
    let formed = matches!(call.kind, CallKind::Do | CallKind::Look);
    if let Some(title) = call.title.as_ref().filter(|_| formed) {
        let body = trim_blank_lines(&call.question);
        let body = (!body.is_empty()).then(|| body.to_owned());
        return (title.clone(), body);
    }
    let mut offset = 0;
    while let Some(rest) = call.question.get(offset..) {
        let (line, next) = match rest.find('\n') {
            Some(end) => (rest.get(..end).unwrap_or_default(), offset + end + 1),
            None => (rest, call.question.len()),
        };
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            let headline = strip_heading_prefix(trimmed).to_owned();
            let body = trim_blank_lines(call.question.get(next..).unwrap_or_default());
            let body = (!body.is_empty()).then(|| body.to_owned());
            return (headline, body);
        }
        if next == offset {
            break;
        }
        offset = next;
    }
    (String::new(), None)
}

/// The first line of `text` with markdown syntax stripped, for single-line ask previews.
pub(super) fn plain_line(text: &str) -> String {
    let words = sikemux_markdown::words(text);
    (words.lines().map(str::trim))
        .chain(text.lines().map(str::trim))
        .find(|line| !line.is_empty() && !line.starts_with("```"))
        .unwrap_or_default()
        .to_owned()
}

/// One-line plain preview of `call`'s question, stripping markdown when the core's headline is
/// absent.
pub(crate) fn plain_headline(call: &PersonCall) -> String {
    if !call.headline.trim().is_empty() {
        return call.headline.clone();
    }
    plain_line(&call.question)
}

/// One-line plain name of `call`: its title when set, else its stripped headline.
pub(crate) fn plain_name(call: &PersonCall) -> String {
    call.title
        .as_deref()
        .map(plain_line)
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| plain_headline(call))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call_with(question: &str, headline: &str, title: Option<&str>) -> PersonCall {
        serde_json::from_value(serde_json::json!({
            "id": "ask-1",
            "project": "/sikemux",
            "kind": "taste",
            "title": title,
            "asker": { "agentId": "chief", "name": "Chief", "cli": "claude" },
            "question": question,
            "headline": headline,
            "options": [],
            "canHandBack": false,
            "state": "open",
            "raisedAt": 1,
        }))
        .unwrap()
    }

    #[test]
    fn splits_and_caches_markdown_question_for_popover_and_page() {
        let question = "Sign up for [GeekNews](https://news.hada.io) and [TabNews](https://www.tabnews.com.br) so `@Masaki` can publish both held launch drafts?\n\n**Pick:** Sign up as `sikemux` with `contact@nodelike.com`.\n\n**Against:** Takes ~2 minutes.";
        let call = call_with(question, "", None);

        let (headline, body) = question_parts(&call);
        assert_eq!(
            headline,
            "Sign up for [GeekNews](https://news.hada.io) and [TabNews](https://www.tabnews.com.br) so `@Masaki` can publish both held launch drafts?"
        );
        assert_eq!(
            body.as_deref(),
            Some(
                "**Pick:** Sign up as `sikemux` with `contact@nodelike.com`.\n\n**Against:** Takes ~2 minutes."
            )
        );

        let mut cache = None;
        let (first, changed) = RenderedCall::sync(&mut cache, &call);
        assert!(changed);
        assert!(first.body.is_some());
        let (second, changed) = RenderedCall::sync(&mut cache, &call);
        assert!(!changed);
        assert!(Rc::ptr_eq(&first, &second));
    }

    #[test]
    fn plain_preview_strips_markdown_links_code_and_bold() {
        let question = "Sign up for [GeekNews](https://news.hada.io) and [TabNews](https://www.tabnews.com.br) so `@Masaki` can publish?\n\n**Pick:** Sign up as `sikemux`.";
        let call = call_with(question, "", None);
        assert_eq!(
            plain_headline(&call),
            "Sign up for GeekNews and TabNews so @Masaki can publish?"
        );
        assert_eq!(
            plain_name(&call),
            "Sign up for GeekNews and TabNews so @Masaki can publish?"
        );

        let titled = call_with("**Pick:** Use `redis`", "", Some("**Cache** `store`"));
        assert_eq!(plain_name(&titled), "Cache store");
    }
}
