//! A Mermaid block in an agent's reply, drawn as its diagram once the agent has finished
//! writing it, with its text a click away.

use std::borrow::Cow;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, img, px, size, AnyElement, App, Asset, ElementId, Entity, Hsla, ImageSource,
    InteractiveElement, IntoElement, ParentElement, Pixels, RenderImage, Rgba, SharedString, Size,
    StatefulInteractiveElement, Styled, Window,
};
use gpui_transcript::Fence;
use resvg::{tiny_skia, usvg};

use super::schema::{choice, head};
use crate::theme::{self, Tokens, SPACE_1H, SPACE_3, SPACE_3H};

pub(super) const LANG: &str = "mermaid";

/// How long a block's text must stay the same while a turn runs before it counts as finished
/// and is drawn, so a diagram is not drawn again with each word the agent writes.
const SETTLED: Duration = Duration::from_millis(400);

/// The most pixels one diagram's picture holds; a larger one is drawn less sharp.
const MOST_PIXELS: f32 = 6_000_000.;

/// What a block keeps between frames: its text as the chat last had it, the text it is drawn
/// from once that stopped changing, the text a wait is under way for, and whether the person
/// chose to see the text.
pub(super) struct Block {
    written: SharedString,
    pub(super) settled: Option<SharedString>,
    waiting: Option<SharedString>,
    pub(super) source: bool,
}

pub(super) fn block(fence: Fence, running: bool, window: &mut Window, cx: &mut App) -> AnyElement {
    let block = kept(&fence, LANG, running, window, cx);
    let t = Tokens::get(cx).clone();
    let drawn = block.read(cx).settled.clone().and_then(|code| {
        let wanted = Wanted {
            code,
            look: Look::of(&t, window.scale_factor()),
        };
        window.use_asset::<Drawn>(&wanted, cx)
    });
    let (drawing, fault) = match drawn {
        Some(Ok(drawing)) => (Some(drawing), None),
        Some(Err(fault)) => (None, (!running).then_some(fault)),
        None => (None, None),
    };
    let source = block.read(cx).source || drawing.is_none();
    let card = crate::reading::code_card(&t);
    let shows = |id: &'static str, label: &'static str, shows_source: bool| {
        let block = block.clone();
        choice(id, label, source == shows_source, &t, move |cx| {
            block.update(cx, |block, cx| {
                block.source = shows_source;
                cx.notify();
            })
        })
    };
    let title = head(fence.icon, fence.lang, &t)
        .when(drawing.is_some(), |title| {
            title
                .child(shows("mermaid-diagram", t!("Diagram"), false))
                .child(shows("mermaid-source", t!("Source"), true))
                .child(div().w(px(6.)))
        })
        .when_some(fault, |title, fault| {
            title.child(
                div()
                    .id("mermaid-fault")
                    .mr(SPACE_1H)
                    .child(t!("Can't draw"))
                    .tooltip(crate::ui::tooltip(fault)),
            )
        })
        .child(fence.copy);
    div()
        .id(ElementId::NamedChild(
            Arc::new(fence.id),
            "mermaid-card".into(),
        ))
        .rounded(crate::theme::corner(card.radius))
        .bg(card.ground)
        .overflow_hidden()
        .child(title)
        .map(|frame| match drawing.filter(|_| !source) {
            None => frame.child(fence.lines),
            Some(drawing) => frame.child(
                div()
                    .px(SPACE_3H)
                    .pt(SPACE_1H)
                    .pb(SPACE_3)
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .w_full()
                            .max_w(drawing.size.width)
                            .aspect_ratio(drawing.size.width / drawing.size.height)
                            .child(img(ImageSource::Render(drawing.image)).size_full()),
                    ),
            ),
        })
        .into_any_element()
}

/// What a block of `fence` keeps between frames, under `lang`: its text is taken as finished
/// once the turn ends, or once it has stayed the same for a moment while the turn runs.
pub(super) fn kept(
    fence: &Fence,
    lang: &'static str,
    running: bool,
    window: &mut Window,
    cx: &mut App,
) -> Entity<Block> {
    let key = ElementId::NamedChild(Arc::new(fence.id.clone()), lang.into());
    let written = fence.code.clone();
    let block = window.use_keyed_state(key, cx, |_, _| Block {
        written: written.clone(),
        settled: None,
        waiting: None,
        source: false,
    });
    let kept = block.read(cx);
    let (unchanged, drawn) = (
        kept.written == written,
        kept.settled.as_ref() == Some(&written),
    );
    let waiting = kept.waiting.as_ref() == Some(&written);
    if !unchanged || (!drawn && (!running || !waiting)) {
        block.update(cx, |kept, _| {
            kept.written = written.clone();
            match running {
                true => kept.waiting = Some(written.clone()),
                false => kept.settled = Some(written.clone()),
            }
        });
        if running && !waiting {
            settle(&block, written, cx);
        }
    }
    block
}

/// Draws `written` once it has stayed the block's text for a moment.
fn settle(block: &Entity<Block>, written: SharedString, cx: &mut App) {
    let block = block.downgrade();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(SETTLED).await;
        let _ = block.update(cx, |kept, cx| {
            if kept.written == written && kept.settled.as_ref() != Some(&written) {
                kept.settled = Some(written);
                kept.waiting = None;
                cx.notify();
            }
        });
    })
    .detach();
}

/// A diagram's text and the colours and scale it is drawn in.
#[derive(Clone, Hash)]
struct Wanted {
    code: SharedString,
    look: Look,
}

/// The theme's colours as the diagram wears them, already laid over the card's ground.
#[derive(Clone, Hash)]
struct Look {
    dark: bool,
    ink: String,
    muted: String,
    ground: String,
    node: String,
    edge: String,
    rule: String,
    cluster: String,
    note: String,
    note_edge: String,
    /// The screen's scale, in hundredths.
    scale: u32,
}

impl Look {
    fn of(t: &Tokens, scale: f32) -> Look {
        let card = crate::reading::code_card(t);
        let ground = theme::over(card.ground, t.surface);
        let tint =
            |colour: Hsla, share: f32| hex(theme::mix(theme::over(colour, ground), ground, share));
        Look {
            dark: t.dark,
            ink: hex(theme::over(t.text_primary, ground)),
            muted: hex(theme::over(t.text_tertiary, ground)),
            ground: hex(ground),
            node: tint(t.text_primary, 0.06),
            edge: hex(theme::over(t.text_tertiary, ground)),
            rule: hex(theme::over(t.border_strong, ground)),
            cluster: tint(t.text_primary, 0.025),
            note: tint(t.warn, 0.14),
            note_edge: tint(t.warn, 0.45),
            scale: (scale * 100.).round() as u32,
        }
    }

    fn theme(&self) -> mermaid_svg::Theme {
        let base = match self.dark {
            true => mermaid_svg::Theme::dark(),
            false => mermaid_svg::Theme::default_theme(),
        };
        let own = |value: &String| Cow::Owned(value.clone());
        mermaid_svg::Theme {
            fg: own(&self.ink),
            fg_muted: own(&self.muted),
            bg: own(&self.ground),
            actor_fill: own(&self.node),
            actor_stroke: own(&self.rule),
            lifeline: own(&self.rule),
            arrow_stroke: own(&self.edge),
            note_fill: own(&self.note),
            note_stroke: own(&self.note_edge),
            activation_fill: own(&self.cluster),
            activation_stroke: own(&self.rule),
            frame_label_fill: own(&self.node),
            flow_node_fill: own(&self.node),
            flow_node_stroke: own(&self.rule),
            flow_edge_stroke: own(&self.edge),
            flow_label_bg: own(&self.ground),
            flow_cluster_fill: own(&self.cluster),
            flow_cluster_stroke: own(&self.rule),
            font_family: Cow::Borrowed(theme::UI),
            font_size: 13.,
            responsive: false,
            ..base
        }
    }
}

fn hex(colour: Hsla) -> String {
    let colour = Rgba::from(colour);
    let byte = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        byte(colour.r),
        byte(colour.g),
        byte(colour.b)
    )
}

/// A drawing made, and its size on screen at full size.
#[derive(Clone)]
pub(super) struct Drawing {
    pub(super) image: Arc<RenderImage>,
    pub(super) size: Size<Pixels>,
}

/// GPUI's loader for a diagram, which draws it off the main thread once for each text and look.
enum Drawn {}

impl Asset for Drawn {
    type Source = Wanted;
    type Output = Result<Drawing, SharedString>;

    #[allow(clippy::manual_async_fn)]
    fn load(
        wanted: Wanted,
        _: &mut App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        async move { draw(&wanted).map_err(SharedString::from) }
    }
}

/// The fonts a diagram's text is set in: the interface's, then the system's for any letter
/// it lacks.
static FONTS: LazyLock<Arc<usvg::fontdb::Database>> = LazyLock::new(|| {
    let mut fonts = usvg::fontdb::Database::new();
    fonts.load_system_fonts();
    fonts.load_font_data(theme::UI_REGULAR.to_vec());
    fonts.load_font_data(theme::UI_SEMIBOLD.to_vec());
    fonts.set_sans_serif_family(theme::UI);
    Arc::new(fonts)
});

fn draw(wanted: &Wanted) -> Result<Drawing, String> {
    let svg = mermaid_svg::render_with(&wanted.code, &wanted.look.theme())
        .map_err(|fault| fault.to_string())?;
    let options = usvg::Options {
        font_family: theme::UI.to_owned(),
        fontdb: FONTS.clone(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(&svg, &options).map_err(|fault| fault.to_string())?;
    picture(&tree, wanted.look.scale as f32 / 100.)
}

/// `tree` drawn at the screen's `scale`, with all its ink, even where it spills past the size
/// it says it is.
pub(super) fn picture(tree: &usvg::Tree, scale: f32) -> Result<Drawing, String> {
    let inked = tree.root().abs_stroke_bounding_box();
    let (left, top) = (inked.left().min(0.), inked.top().min(0.));
    let right = inked.right().max(tree.size().width());
    let bottom = inked.bottom().max(tree.size().height());
    let (width, height) = (right - left + 2., bottom - top + 2.);
    let scale = scale.min((MOST_PIXELS / (width * height)).sqrt()).max(0.01);
    let mut pixmap = tiny_skia::Pixmap::new(
        (width * scale).ceil() as u32,
        (height * scale).ceil() as u32,
    )
    .ok_or(t!("The drawing is empty"))?;
    let place = tiny_skia::Transform::from_row(
        scale,
        0.,
        0.,
        scale,
        (1. - left) * scale,
        (1. - top) * scale,
    );
    resvg::render(tree, place, &mut pixmap.as_mut());
    let (columns, rows) = (pixmap.width(), pixmap.height());
    let mut pixels = image::RgbaImage::from_raw(columns, rows, pixmap.take())
        .ok_or(t!("The drawing could not be made"))?;
    for pixel in pixels.as_chunks_mut::<4>().0 {
        gpui::swap_rgba_pa_to_bgra(pixel);
    }
    Ok(Drawing {
        image: Arc::new(RenderImage::new([image::Frame::new(pixels)])),
        size: size(px(width), px(height)),
    })
}
