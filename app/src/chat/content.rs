//! What an agent put in its reply besides text, as its protocol gave it: a picture, or a
//! link to a file or page.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, WeakEntity,
};
use gpui_transcript::FilePress;
use sikemux_wire::chat::ContentBlock;
use tokio::sync::mpsc::UnboundedSender;

use super::{link, Drawn};
use crate::core::Ask;
use crate::icons::Icon;
use crate::picture::{self, Picture, Remote};
use crate::theme::{SPACE_2, TEXT_2XS, TEXT_XS_H};
use crate::ui;
use crate::workbench::Workbench;

const WIDTH: f32 = 620.;
const HEIGHT: f32 = 440.;

/// A picture a reply's markdown shows, which opens over the window when clicked: a file on
/// this host, or a page's picture the core fetches.
pub fn image(
    src: &str,
    bench: &WeakEntity<Workbench>,
    asks: &UnboundedSender<Ask>,
    quiet: Hsla,
) -> AnyElement {
    let picture = match link::local_path(src) {
        Some(path) => Picture::File(path),
        None => Picture::Remote(Remote::new(src.to_owned(), asks.clone())),
    };
    picture::thumb(
        SharedString::from(format!("image:{src}")),
        picture,
        px(WIDTH),
        px(HEIGHT),
        bench.clone(),
        quiet,
    )
    .into_any_element()
}

fn filled(text: &str) -> Option<&str> {
    Some(text).filter(|text| !text.is_empty())
}

pub fn content(
    id: SharedString,
    content: &ContentBlock,
    looks: &Drawn,
    cx: &mut App,
) -> AnyElement {
    let (uri, label) = match content {
        ContentBlock::Image { uri, .. } => (uri.as_deref().and_then(filled), None),
        ContentBlock::ResourceLink {
            uri, name, title, ..
        } => (
            filled(uri),
            title.as_deref().and_then(filled).or(filled(name)),
        ),
        _ => (None, None),
    };
    let picture = looks
        .pictures
        .borrow_mut()
        .entry(id.clone())
        .or_insert_with(|| match content {
            ContentBlock::Image {
                mime_type, data, ..
            } => Picture::sent(filled(mime_type)?, data.as_deref().and_then(filled)?),
            _ => link::local_path(uri?)
                .filter(|path| super::attach::is_image(path))
                .map(Picture::File),
        })
        .clone();
    if let Some(picture) = picture {
        return div()
            .py(SPACE_2)
            .child(picture::thumb(
                id,
                picture,
                px(WIDTH),
                px(HEIGHT),
                looks.bench.clone(),
                looks.quiet,
            ))
            .into_any_element();
    }
    if !matches!(
        content,
        ContentBlock::Image { .. } | ContentBlock::ResourceLink { .. }
    ) {
        let shown = match content {
            ContentBlock::Other { block } => serde_json::to_string_pretty(block),
            block => serde_json::to_string_pretty(block),
        };
        return ui::mono(shown.unwrap_or_default(), TEXT_2XS, cx)
            .text_color(looks.quiet)
            .whitespace_normal()
            .into_any_element();
    }
    let (uri, ink) = (uri.map(str::to_owned), looks.ink);
    let (bench, agent) = (looks.bench.clone(), looks.agent.clone());
    let label = label.or(uri.as_deref()).unwrap_or("Resource").to_owned();
    let file = uri
        .as_deref()
        .is_some_and(|uri| super::files::Files::kind_of(&looks.files, uri, cx).is_some());
    let pressed = looks.file_pressed();
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(7.))
        .min_w_0()
        .text_size(crate::theme::ui_text(TEXT_XS_H))
        .text_color(if file { looks.accent } else { looks.quiet })
        .when_some(uri.clone().filter(|_| file), |row, uri| {
            let menu = (uri.clone(), pressed.clone());
            row.cursor_pointer()
                .on_click(move |event, window, cx| {
                    let press = if event.click_count() > 1 {
                        FilePress::OpenTwice
                    } else {
                        FilePress::Open
                    };
                    pressed(&uri, press, window, cx)
                })
                .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                    (menu.1)(&menu.0, FilePress::Menu(event.position), window, cx)
                })
        })
        .when_some(uri.filter(|_| !file), |row, uri| {
            row.cursor_pointer()
                .hover(move |row| row.text_color(ink))
                .on_click(move |_, window, cx| link::open(&uri, &agent, &bench, window, cx))
        })
        .child(ui::icon(Icon::File, 13.).text_color(looks.quiet))
        .child(div().min_w_0().truncate().child(label))
        .into_any_element()
}
