use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::ops::Range;
use std::panic::Location;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder;
use gpui::{
    canvas, combine_highlights, div, img, point, px, relative, rgb, rgba, size, AnyElement, App,
    AvailableSpace, Bounds, ClipboardItem, Div, Element, ElementId, Font, FontStyle, FontWeight,
    GlobalElementId, HighlightStyle, Hsla, InspectorElementId, InteractiveElement, InteractiveText,
    IntoElement, LayoutId, MouseButton, ParentElement, Pixels, Point, SharedString, Stateful,
    StatefulInteractiveElement, StrikethroughStyle, Styled, StyledText, Window,
};

use crate::markdown::{superscript, Kind, Markdown, Marks, Node, Piece, Rich};
use crate::select::{Key, Selecting, Shown};

/// A fenced code block, for a host that draws its own.
pub struct Fence {
    pub id: ElementId,
    pub lang: Option<SharedString>,
    pub code: SharedString,
}

pub type OnLink = Rc<dyn Fn(&str, &mut Window, &mut App)>;
/// Ticks or clears a task: its place among the text's task boxes, its words, and whether it
/// is now done.
pub type OnTask = Rc<dyn Fn(usize, &str, bool, &mut Window, &mut App)>;
pub type DrawFence = Rc<dyn Fn(Fence, &mut Window, &mut App) -> AnyElement>;
/// Styled runs over a code block's text, in bytes from its start.
pub type CodeRuns = Rc<[(Range<usize>, HighlightStyle)]>;
/// A fenced code block's colours from its language and text, once the host has them.
pub type CodeColours =
    Rc<dyn Fn(Option<&SharedString>, &SharedString, &mut App) -> Option<CodeRuns>>;
/// Draws a picture from its address and its alt text.
pub type DrawImage = Rc<dyn Fn(&str, &str, &mut Window, &mut App) -> AnyElement>;

/// What a name the message wrote turned out to be, once the host looked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    File,
    Folder,
}

/// How the reader reached for a file the message names.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FilePress {
    /// One click.
    Open,
    /// Two clicks.
    OpenTwice,
    /// The menu, where the pointer is.
    Menu(Point<Pixels>),
}

/// Whether a name the message wrote is a file the host holds, none until the host has said.
pub type FileOf = Rc<dyn Fn(&str, &mut App) -> Option<FileKind>>;
/// What reaching for a file does, by its name as written.
pub type OnFile = Rc<dyn Fn(&str, FileKind, FilePress, &mut Window, &mut App)>;
/// The icon drawn inside the text before a file's name, by its name as written, `size` square.
pub type FileIcon = Rc<dyn Fn(&str, FileKind, Pixels, &mut Window, &mut App) -> Option<AnyElement>>;

/// A file's icon beside its name, in ems of the text it sits in, and the space after it.
const FILE_ICON_EMS: f32 = 0.95;
const FILE_ICON_SPACE_EMS: f32 = 0.3;

/// How markdown looks and what it does when clicked. The defaults are Sikemux's dark theme.
#[derive(Clone)]
pub struct Style {
    pub font: SharedString,
    pub mono: Font,
    pub text_size: Pixels,
    pub line_height: f32,
    pub code_size: Pixels,
    pub table_size: Pixels,
    pub gap: Pixels,
    pub ink: Hsla,
    pub ink_dim: Hsla,
    pub ink_faint: Hsla,
    pub accent: Hsla,
    pub code_ink: Hsla,
    pub code_ground: Hsla,
    pub mark_ground: Hsla,
    pub selection: Hsla,
    /// The ground under each place a find found, and under the one the reader is on.
    pub found: Hsla,
    pub found_current: Hsla,
    pub line: Hsla,
    pub line_soft: Hsla,
    pub table_ground: Hsla,
    pub table_head: Hsla,
    pub radius: Pixels,
    pub on_link: Option<OnLink>,
    /// Makes task boxes clickable.
    pub on_task: Option<OnTask>,
    /// Draws the names that are files the host holds as files, and opens them.
    pub file_of: Option<FileOf>,
    pub on_file: Option<OnFile>,
    /// Draws each file's icon inside the text, just before its name.
    pub file_icon: Option<FileIcon>,
    pub fence: Option<DrawFence>,
    pub code: Option<CodeColours>,
    pub image: Option<DrawImage>,
    /// Level-two headings lead with their number, `01` and on, in the accent's mono face.
    pub numbered_sections: bool,
    pub italic: bool,
}

impl Style {
    /// The same look with its text, code, tables and the gaps between blocks `scale` times
    /// their size.
    pub fn scaled(&self, scale: f32) -> Style {
        Style {
            text_size: self.text_size * scale,
            code_size: self.code_size * scale,
            table_size: self.table_size * scale,
            gap: self.gap * scale,
            ..self.clone()
        }
    }
}

impl Default for Style {
    fn default() -> Self {
        Style {
            font: "Figtree".into(),
            mono: gpui::font("JetBrainsMono NFM"),
            text_size: px(14.),
            line_height: 1.65,
            code_size: px(12.),
            table_size: px(13.5),
            gap: px(10.),
            ink: rgb(0xe7e5ef).into(),
            ink_dim: rgb(0x8b8898).into(),
            ink_faint: rgb(0x736f80).into(),
            accent: rgb(0xa277ff).into(),
            code_ink: rgb(0xb494fb).into(),
            code_ground: rgba(0x09090bb8).into(),
            mark_ground: rgba(0xa277ff1a).into(),
            selection: rgba(0xa277ff4d).into(),
            found: rgba(0xa277ff47).into(),
            found_current: rgba(0xa277ffff).into(),
            line: rgba(0x24242bcc).into(),
            line_soft: rgb(0x19191e).into(),
            table_ground: rgba(0x19191eb8).into(),
            table_head: rgba(0xe7e5ef0a).into(),
            radius: px(5.),
            on_link: None,
            on_task: None,
            file_of: None,
            on_file: None,
            file_icon: None,
            fence: None,
            code: None,
            image: None,
            numbered_sections: false,
            italic: false,
        }
    }
}

/// A message of this many blocks or more lays out only the blocks in view,
/// once each block's size is known.
const TALL: usize = 12;

/// What draws a row's markdown: the style, and in a transcript, the row's
/// selection and where its text is recorded so the pointer can select it.
pub struct Pen {
    style: Rc<Style>,
    selecting: Option<Rc<Selecting>>,
}

impl Pen {
    pub fn new(style: Rc<Style>) -> Pen {
        Pen::selecting(style, None)
    }

    pub(crate) fn selecting(style: Rc<Style>, selecting: Option<Selecting>) -> Pen {
        Pen {
            style,
            selecting: selecting.map(Rc::new),
        }
    }

    pub fn style(&self) -> &Style {
        &self.style
    }

    /// Draws a message's markdown. `id` must be unique in the window. In a
    /// transcript, `doc` is one of those `Row::docs` lists, which is how a selection finds it.
    pub fn markdown(
        &self,
        id: impl Into<ElementId>,
        doc: &Markdown,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.markdown_with(&self.style.clone(), id, doc, window, cx)
    }

    /// Draws markdown in a look of its own, such as a thought's quieter one.
    pub fn markdown_with(
        &self,
        style: &Rc<Style>,
        id: impl Into<ElementId>,
        doc: &Markdown,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.blocks_with(style, id, doc, doc.blocks().to_vec(), window, cx)
    }

    /// Draws only `blocks` of `doc`, such as the ones `Markdown::cut` keeps of a long text.
    pub fn blocks_with(
        &self,
        style: &Rc<Style>,
        id: impl Into<ElementId>,
        doc: &Markdown,
        blocks: Vec<Rc<Node>>,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let id = id.into();
        let blocks = Blocks {
            scope: Arc::new(id.clone()),
            blocks,
            style: style.clone(),
            selecting: self.selecting.clone(),
            doc: self
                .selecting
                .as_ref()
                .map_or(0, |selecting| selecting.place_of(doc)),
            width: doc.width.clone(),
            natural: doc.natural.clone(),
            turned: doc.turned.clone(),
            top: doc.top.clone(),
        };
        let root = div()
            .flex()
            .flex_col()
            .gap(style.gap)
            .min_w_0()
            .font_family(style.font.clone())
            .when(style.italic, |root| root.italic())
            .text_size(style.text_size)
            .line_height(relative(style.line_height))
            .text_color(style.ink);
        if blocks.blocks.len() >= TALL && blocks.width.get().is_some() {
            return root.child(Tall(blocks)).id(id).into_any_element();
        }
        let children: Vec<_> = (0..blocks.blocks.len())
            .map(|ix| blocks.draw(ix, window, cx))
            .collect();
        root.children(children)
            .on_children_prepainted(move |bounds, _, _| {
                for (block, bounds) in blocks.blocks.iter().zip(&bounds) {
                    block.size.set(Some(bounds.size));
                }
                if let Some(first) = bounds.first() {
                    blocks.width.set(Some(first.size.width));
                    blocks.top.set(Some(first.top()));
                    blocks.natural.set(Some(first.size.width));
                }
            })
            .id(id)
            .into_any_element()
    }
}

/// One message's blocks and what drawing them needs.
struct Blocks {
    scope: Arc<ElementId>,
    blocks: Vec<Rc<Node>>,
    style: Rc<Style>,
    selecting: Option<Rc<Selecting>>,
    doc: usize,
    width: Rc<Cell<Option<Pixels>>>,
    natural: Rc<Cell<Option<Pixels>>>,
    turned: Rc<RefCell<HashSet<(usize, u32)>>>,
    top: Rc<Cell<Option<Pixels>>>,
}

impl Blocks {
    fn draw(&self, ix: usize, window: &mut Window, cx: &mut App) -> AnyElement {
        let tasks = match self.style.on_task {
            Some(_) => self.blocks[..ix].iter().map(|block| tasks_in(block)).sum(),
            None => 0,
        };
        let mut draw = Draw {
            on: self,
            block: ix,
            next: 0,
            texts: 0,
            folds: 0,
            depth: 0,
            tasks,
        };
        draw.block(&self.blocks[ix], ix, window, cx)
    }

    fn laid_out(&self, ix: usize, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
        let mut element = self.draw(ix, window, cx);
        let space = size(AvailableSpace::Definite(width), AvailableSpace::MinContent);
        let height = element.layout_as_root(space, window, cx).height;
        self.blocks[ix].size.set(Some(size(width, height)));
        element
    }
}

/// The blocks of a tall message: each laid out only when it changed or the
/// width did, and placed only when it shows.
struct Tall(Blocks);

impl IntoElement for Tall {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Tall {
    type RequestLayoutState = Vec<Option<AnyElement>>;
    type PrepaintState = Vec<AnyElement>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let blocks = &self.0;
        let width = blocks.width.get().unwrap_or_default();
        let laid: Vec<_> = (0..blocks.blocks.len())
            .map(|ix| {
                let fresh = blocks.blocks[ix]
                    .size
                    .get()
                    .is_some_and(|size| size.width == width);
                (!fresh).then(|| blocks.laid_out(ix, width, window, cx))
            })
            .collect();
        let heights = blocks
            .blocks
            .iter()
            .map(|block| block.size.get().map_or(px(0.), |size| size.height));
        let gaps = blocks.style.gap * blocks.blocks.len().saturating_sub(1) as f32;
        let height = heights.fold(gaps, |sum, height| sum + height);
        // A parent that sizes itself to its content, as a sent message's bubble does, asks for
        // a width. Answering with the width it was last given would narrow it by the parent's
        // padding every frame, so it is the width the blocks first took, as far as there is room.
        let natural = blocks.natural.get().unwrap_or(width);
        let layout = window.request_measured_layout(
            gpui::Style::default(),
            move |known, available, _, _| {
                let room = match available.width {
                    AvailableSpace::Definite(room) => room.min(natural),
                    _ => natural,
                };
                size(known.width.unwrap_or(room), known.height.unwrap_or(height))
            },
        );
        (layout, laid)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        laid: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let blocks = &self.0;
        let width = bounds.size.width;
        if blocks.width.replace(Some(width)) != Some(width) {
            window.refresh();
        }
        blocks.top.set(Some(bounds.top()));
        let view = window.content_mask().bounds;
        let mut top = bounds.top();
        let mut shown = Vec::new();
        for (ix, block) in blocks.blocks.iter().enumerate() {
            let height = block.size.get().map_or(px(0.), |size| size.height);
            if top + height >= view.top() && top <= view.bottom() {
                let mut element = laid[ix]
                    .take()
                    .unwrap_or_else(|| blocks.laid_out(ix, width, window, cx));
                element.prepaint_at(point(bounds.left(), top), window, cx);
                shown.push(element);
            }
            top += height + blocks.style.gap;
        }
        shown
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        shown: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        for element in shown {
            element.paint(window, cx);
        }
    }
}

/// How a text's bytes are drawn, by range.
type Looks = Vec<(Range<usize>, HighlightStyle)>;

/// How many lines of a code block are laid out together.
const CODE_CHUNK: usize = 8;
/// How long a frame may spend laying out parts of code blocks just past the view, ahead of their
/// scrolling in; parts in view are always laid out.
const CODE_LAYOUT_BUDGET: Duration = Duration::from_millis(4);

/// What a transcript's frame has spent so far laying out new parts of its code blocks.
#[derive(Default)]
pub(crate) struct LayoutBudget {
    spent: Cell<Duration>,
}

impl LayoutBudget {
    pub(crate) fn reset(&self) {
        self.spent.set(Duration::ZERO);
    }

    fn has_room(&self) -> bool {
        self.spent.get() < CODE_LAYOUT_BUDGET
    }

    fn spend(&self, time: Duration) {
        self.spent.set(self.spent.get() + time);
    }
}

/// A code block's text, which does not wrap, so each line's place is known before it is laid
/// out: only the lines in view are, a few at a time, as a long block shows only some in its box.
struct Code {
    id: ElementId,
    text: SharedString,
    /// Where each line starts.
    starts: Vec<usize>,
    looks: Looks,
    shown: Option<(Key, Rc<RefCell<Vec<Shown>>>)>,
    /// A transcript's frame budget for new parts; without one every part is laid out at once.
    budget: Option<Rc<LayoutBudget>>,
}

impl Code {
    /// Part `chunk` as text to lay out, with the lines it holds and where it starts in the block.
    fn part(&self, chunk: usize) -> Option<(StyledText, std::ops::Range<usize>, usize, usize)> {
        let lines = chunk * CODE_CHUNK..((chunk + 1) * CODE_CHUNK).min(self.starts.len());
        let start = *self.starts.get(lines.start)?;
        let end = self
            .starts
            .get(lines.end)
            .map_or(self.text.len(), |next| next - 1);
        let looks = self.looks.iter().filter_map(|(range, look)| {
            let (from, to) = (range.start.max(start), range.end.min(end));
            (from < to).then(|| (from - start..to - start, *look))
        });
        let text = StyledText::new(SharedString::from(self.text[start..end].to_owned()))
            .with_highlights(looks);
        Some((text, lines, start, end))
    }

    /// Lays out and places the parts in view, then, while the frame's budget allows, lays out
    /// those just past the view in either direction, nearest first, so they are ready when they
    /// scroll in.
    fn lay_out(
        &self,
        visible: std::ops::RangeInclusive<usize>,
        bounds: Bounds<Pixels>,
        line_height: Pixels,
        laid: &mut HashSet<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        let space = size(AvailableSpace::MaxContent, AvailableSpace::MinContent);
        let mut drawn = Vec::new();
        for chunk in visible.clone() {
            let began = Instant::now();
            let Some((text, lines, start, end)) = self.part(chunk) else {
                break;
            };
            let layout = text.layout().clone();
            let mut element = text.into_any_element();
            element.layout_as_root(space, window, cx);
            let top = bounds.top() + line_height * lines.start as f32;
            element.prepaint_at(point(bounds.left(), top), window, cx);
            drawn.push(element);
            if laid.insert(chunk) {
                if let Some(budget) = &self.budget {
                    budget.spend(began.elapsed());
                }
                #[cfg(feature = "counts")]
                {
                    crate::counts::add(&crate::counts::NEW_CODE_LINES, lines.len() as u64);
                    let micros = began.elapsed().as_micros() as u64;
                    crate::counts::add(&crate::counts::NEW_CODE_US, micros);
                }
            }
            #[cfg(feature = "counts")]
            {
                crate::counts::add(&crate::counts::CODE_CHUNKS, 1);
                crate::counts::add(&crate::counts::CODE_LINES, lines.len() as u64);
                crate::counts::add(&crate::counts::CODE_US, began.elapsed().as_micros() as u64);
            }
            if let Some((key, shown)) = &self.shown {
                shown.borrow_mut().push(Shown {
                    key: key.clone(),
                    layout,
                    len: end - start,
                    offset: start,
                });
            }
        }
        let Some(budget) = &self.budget else {
            return drawn;
        };
        let (first, last) = (*visible.start(), *visible.end());
        let reach = last - first + 1;
        let parts = self.starts.len().div_ceil(CODE_CHUNK);
        laid.retain(|chunk| *chunk + reach >= first && *chunk <= last + reach);
        let near = (1..=reach).flat_map(|away| [last.checked_add(away), first.checked_sub(away)]);
        for chunk in near.flatten().filter(|chunk| *chunk < parts) {
            let warm = laid.contains(&chunk);
            if !warm && !budget.has_room() {
                continue;
            }
            let began = Instant::now();
            let Some((text, _lines, ..)) = self.part(chunk) else {
                continue;
            };
            text.into_any_element().layout_as_root(space, window, cx);
            if !warm {
                laid.insert(chunk);
                budget.spend(began.elapsed());
                #[cfg(feature = "counts")]
                {
                    crate::counts::add(&crate::counts::NEW_CODE_LINES, _lines.len() as u64);
                    let micros = began.elapsed().as_micros() as u64;
                    crate::counts::add(&crate::counts::NEW_CODE_US, micros);
                }
            }
            #[cfg(feature = "counts")]
            crate::counts::add(&crate::counts::CODE_US, began.elapsed().as_micros() as u64);
        }
        drawn
    }
}

impl IntoElement for Code {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Code {
    type RequestLayoutState = Pixels;
    type PrepaintState = Vec<AnyElement>;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        _: &mut App,
    ) -> (LayoutId, Pixels) {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.pixel_snap(
            style
                .line_height
                .to_pixels(font_size.into(), window.rem_size()),
        );
        let font = window.text_system().resolve_font(&style.font());
        let advance = (window.text_system().advance(font, font_size, 'm'))
            .map_or(font_size * 0.6, |advance| advance.width);
        let widest = self.text.split('\n').map(|line| line.chars().count()).max();
        let width = advance * widest.unwrap_or(0) as f32;
        let height = line_height * self.starts.len() as f32;
        let layout =
            window.request_measured_layout(gpui::Style::default(), move |known, _, _, _| {
                size(known.width.unwrap_or(width), known.height.unwrap_or(height))
            });
        (layout, line_height)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        line_height: &mut Pixels,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        let line_height = *line_height;
        let starts = &self.starts;
        let view = window.content_mask().bounds;
        let line_at = |y: Pixels| ((y - bounds.top()) / line_height).max(0.) as usize;
        let first = line_at(view.top()) / CODE_CHUNK;
        let last = line_at(view.bottom()).min(starts.len().saturating_sub(1)) / CODE_CHUNK;
        if view.bottom() < bounds.top() || view.top() > bounds.bottom() {
            return Vec::new();
        }
        match id {
            Some(id) => window.with_element_state::<HashSet<usize>, _>(id, |laid, window| {
                let mut laid = laid.unwrap_or_default();
                let drawn = self.lay_out(first..=last, bounds, line_height, &mut laid, window, cx);
                (drawn, laid)
            }),
            None => self.lay_out(
                first..=last,
                bounds,
                line_height,
                &mut HashSet::new(),
                window,
                cx,
            ),
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Pixels,
        drawn: &mut Vec<AnyElement>,
        window: &mut Window,
        cx: &mut App,
    ) {
        for element in drawn {
            element.paint(window, cx);
        }
    }
}

/// Draws one block. The texts it draws and the folds it meets are counted in
/// the order `Node::walk` lists them, so a selection's places match the copy.
struct Draw<'a> {
    on: &'a Blocks,
    block: usize,
    next: u64,
    texts: u32,
    folds: u32,
    depth: usize,
    /// Task boxes drawn before the next, in the whole text.
    tasks: usize,
}

impl Draw<'_> {
    fn id(&mut self, name: &'static str) -> ElementId {
        self.next += 1;
        let name = format!("{}.{name}{}", self.block, self.next);
        ElementId::NamedChild(self.on.scope.clone(), name.into())
    }

    fn block(&mut self, node: &Node, ix: usize, window: &mut Window, cx: &mut App) -> AnyElement {
        let style = &*self.on.style;
        match &node.kind {
            Kind::Heading(level) => {
                let scale = [2., 1.5, 1.17, 1., 0.83, 0.67][usize::from(level.clamp(&1, &6) - 1)];
                let blocks = &self.on.blocks;
                let top_level = blocks
                    .get(ix)
                    .is_some_and(|block| std::ptr::eq(&**block, node));
                let number = (style.numbered_sections && *level == 2 && top_level).then(|| {
                    let before = blocks[..ix]
                        .iter()
                        .filter(|block| matches!(block.kind, Kind::Heading(2)))
                        .count();
                    format!("{:02}", before + 1)
                });
                div()
                    .when(ix > 0, |heading| {
                        heading.pt(style.text_size * (scale * 0.4))
                    })
                    .text_size(style.text_size * scale)
                    .line_height(relative(1.3))
                    .font_weight(FontWeight::SEMIBOLD)
                    .when_some(number, |heading, number| {
                        heading.flex().items_baseline().gap(px(10.)).child(
                            div()
                                .flex_none()
                                .font(style.mono.clone())
                                .text_size(style.code_size * 0.92)
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(style.accent)
                                .child(number),
                        )
                    })
                    .child(self.flow(node, window, cx))
                    .into_any_element()
            }
            Kind::Quote => div()
                .pl(px(12.))
                .border_l_2()
                .border_color(style.line)
                .text_color(style.ink_dim)
                .child(self.flow(node, window, cx))
                .into_any_element(),
            Kind::Bullets | Kind::Numbers(_) => self.list(node, window, cx),
            Kind::Code(lang) => self.fence(node, lang.clone(), window, cx),
            Kind::Rule => div().h(px(1.)).my(px(4.)).bg(style.line).into_any_element(),
            Kind::Table(_) => self.table(node, cx),
            Kind::Footnote(n) => div()
                .flex()
                .gap(px(4.))
                .text_color(style.ink_dim)
                .child(superscript(*n))
                .child(self.flow(node, window, cx))
                .into_any_element(),
            Kind::Details { summary, open } => self.details(node, summary, *open, window, cx),
            Kind::Image { src, alt } => match &style.image {
                Some(image) => image(src, alt, window, cx),
                None => img(src.clone()).max_w_full().into_any_element(),
            },
            _ => self.flow(node, window, cx),
        }
    }

    /// An element's content: its inline runs as text, its blocks as blocks.
    fn flow(&mut self, node: &Node, window: &mut Window, cx: &mut App) -> AnyElement {
        let pieces = node.flow();
        if let [Piece::Text(rich)] = pieces {
            return self.rich(rich, cx);
        }
        let children: Vec<_> = pieces
            .iter()
            .map(|piece| match piece {
                Piece::Text(rich) => self.rich(rich, cx),
                Piece::Block(ix) => self.block(&node.children[*ix], *ix, window, cx),
            })
            .collect();
        let gap = self.on.style.gap;
        div()
            .flex()
            .flex_col()
            .gap(gap)
            .min_w_0()
            .children(children)
            .into_any_element()
    }

    fn rich(&mut self, rich: &Rich, cx: &mut App) -> AnyElement {
        self.coloured(rich, None, cx)
    }

    /// The next text's place among those drawn, and its own marks with the selection and the
    /// places found over them.
    fn marked(&mut self, rich: &Rich) -> ((usize, usize, u32), Looks) {
        let on = self.on;
        self.texts += 1;
        let place = (on.doc, self.block, self.texts);
        let selected = on.selecting.as_ref().and_then(|selecting| {
            let span = selecting.span?;
            span.range((selecting.ix, place.0, place.1, place.2), rich.text.len())
        });
        let found: Vec<(Range<usize>, bool)> = on
            .selecting
            .as_ref()
            .map(|selecting| {
                selecting
                    .hits
                    .iter()
                    .filter(|hit| hit.text == place)
                    .map(|hit| (hit.range.clone(), hit.current))
                    .collect()
            })
            .unwrap_or_default();
        (place, highlights(rich, selected, &found, &on.style))
    }

    /// A code block's text with `colours` beneath the selection, its lines drawn as they show.
    fn code(&mut self, rich: &Rich, colours: Option<CodeRuns>) -> AnyElement {
        let (place, own) = self.marked(rich);
        let looks = match colours {
            Some(colours) => combine_highlights(colours.iter().cloned(), own).collect(),
            None => own,
        };
        let selecting = self.on.selecting.as_ref();
        let shown = selecting.map(|selecting| {
            let key = (selecting.row.clone(), place.0, place.1, place.2);
            (key, selecting.shown.clone())
        });
        Code {
            id: self.id("lines"),
            starts: std::iter::once(0)
                .chain(rich.text.match_indices('\n').map(|(at, _)| at + 1))
                .collect(),
            text: rich.text.clone(),
            looks,
            shown,
            budget: selecting.map(|selecting| selecting.budget.clone()),
        }
        .into_any_element()
    }

    /// Text with `colours` beneath its own marks and the selection.
    fn coloured(&mut self, rich: &Rich, colours: Option<CodeRuns>, cx: &mut App) -> AnyElement {
        let on = self.on;
        let style = &*on.style;
        let (place, own) = self.marked(rich);
        let mono = rich
            .marks
            .iter()
            .filter(|(_, marks)| marks.has(Marks::CODE));
        let files: Vec<(Range<usize>, SharedString, FileKind)> = match &style.file_of {
            Some(file_of) => rich
                .paths
                .iter()
                .chain(&rich.links)
                .filter_map(|(range, name)| Some((range.clone(), name.clone(), file_of(name, cx)?)))
                .collect(),
            None => Vec::new(),
        };
        let file_looks = files.iter().map(|(range, _, _)| {
            let look = HighlightStyle {
                color: Some(style.accent),
                ..HighlightStyle::default()
            };
            (range.clone(), look)
        });
        let own: Vec<_> = combine_highlights(file_looks, own).collect();
        let looks = match colours {
            Some(colours) => combine_highlights(colours.iter().cloned(), own).collect(),
            None => own,
        };
        let icon_size = style.text_size * FILE_ICON_EMS;
        let mut gaps: Vec<(usize, Pixels)> = match &style.file_icon {
            Some(_) => files
                .iter()
                .map(|(range, _, _)| {
                    (
                        range.start,
                        icon_size + style.text_size * FILE_ICON_SPACE_EMS,
                    )
                })
                .collect(),
            None => Vec::new(),
        };
        gaps.sort_by_key(|(at, _)| *at);
        gaps.dedup_by_key(|(at, _)| *at);
        #[cfg(feature = "counts")]
        crate::counts::add(&crate::counts::TEXTS, 1);
        let text = StyledText::new(rich.text.clone())
            .with_highlights(looks)
            .with_font_family_overrides(
                mono.map(|(range, _)| (range.clone(), style.mono.family.clone())),
            )
            .with_gaps(gaps);
        let layout = text.layout().clone();
        let links: Vec<&(Range<usize>, SharedString)> = rich
            .links
            .iter()
            .filter(|(range, _)| !files.iter().any(|(file, _, _)| file == range))
            .collect();
        let inner = if links.is_empty() {
            text.into_any_element()
        } else {
            let ranges = links.iter().map(|(range, _)| range.clone()).collect();
            let links: Rc<[SharedString]> = links.iter().map(|(_, href)| href.clone()).collect();
            let on_link = style.on_link.clone();
            InteractiveText::new(self.id("text"), text)
                .on_click(ranges, move |ix, window, cx| {
                    match (&on_link, links.get(ix)) {
                        (Some(on_link), Some(href)) => on_link(href, window, cx),
                        (None, Some(href)) => cx.open_url(href),
                        _ => {}
                    }
                })
                .into_any_element()
        };
        let inner = match (&style.file_icon, files.is_empty()) {
            (Some(file_icon), false) => {
                let (file_icon, named, layout) = (file_icon.clone(), files.clone(), layout.clone());
                div()
                    .relative()
                    .child(inner)
                    .child(
                        canvas(
                            move |_, window, cx| {
                                let line = layout.line_height();
                                named
                                    .iter()
                                    .filter_map(|(range, name, kind)| {
                                        let at = layout.gap_origin(range.start)?;
                                        let mut icon =
                                            file_icon(name, *kind, icon_size, window, cx)?;
                                        let top = (line - icon_size) / 2.;
                                        icon.prepaint_as_root(
                                            point(at.x, at.y + top),
                                            size(
                                                AvailableSpace::Definite(icon_size),
                                                AvailableSpace::Definite(icon_size),
                                            ),
                                            window,
                                            cx,
                                        );
                                        Some(icon)
                                    })
                                    .collect::<Vec<_>>()
                            },
                            |_, mut icons, window, cx| {
                                for icon in &mut icons {
                                    icon.paint(window, cx);
                                }
                            },
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .into_any_element()
            }
            _ => inner,
        };
        let inner = match (&style.on_file, files.is_empty()) {
            (Some(on_file), false) => {
                let (on_file, files, layout) = (on_file.clone(), Rc::new(files), layout.clone());
                let pressed = move |position: Point<Pixels>, press: FilePress| {
                    let at = layout.index_for_position(position).ok()?;
                    let (_, name, kind) = files.iter().find(|(range, _, _)| range.contains(&at))?;
                    Some((name.clone(), *kind, press))
                };
                let (press, menu) = (Rc::new(pressed), on_file.clone());
                let menu_press = press.clone();
                div()
                    .child(inner)
                    .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                        let twice = if event.click_count > 1 {
                            FilePress::OpenTwice
                        } else {
                            FilePress::Open
                        };
                        if let Some((name, kind, press)) = press(event.position, twice) {
                            cx.stop_propagation();
                            on_file(&name, kind, press, window, cx);
                        }
                    })
                    .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                        let asked = menu_press(event.position, FilePress::Menu(event.position));
                        if let Some((name, kind, press)) = asked {
                            cx.stop_propagation();
                            menu(&name, kind, press, window, cx);
                        }
                    })
                    .into_any_element()
            }
            _ => inner,
        };
        let Some(selecting) = &on.selecting else {
            return inner;
        };
        let key = (selecting.row.clone(), place.0, place.1, place.2);
        let shown = Cell::new(Some(Shown {
            key,
            layout,
            len: rich.text.len(),
            offset: 0,
        }));
        let all = selecting.shown.clone();
        div()
            .child(inner)
            .on_children_prepainted(move |_, _, _| all.borrow_mut().extend(shown.take()))
            .into_any_element()
    }

    fn list(&mut self, node: &Node, window: &mut Window, cx: &mut App) -> AnyElement {
        let style = &*self.on.style;
        let bullet = ["•", "◦", "▪"][self.depth % 3];
        self.depth += 1;
        let items: Vec<_> = node
            .children
            .iter()
            .enumerate()
            .map(|(ix, item)| {
                let marker = match (&item.kind, &node.kind) {
                    (Kind::Item(Some(checked)), _) => {
                        let place = self.tasks;
                        self.tasks += 1;
                        let tick = task_box(*checked, style);
                        match style.on_task.clone() {
                            Some(on_task) => {
                                let (done, words) = (!*checked, task_words(item));
                                tick.id(("task", place))
                                    .cursor_pointer()
                                    .on_click(move |_, window, cx| {
                                        on_task(place, &words, done, window, cx)
                                    })
                                    .into_any_element()
                            }
                            None => tick.into_any_element(),
                        }
                    }
                    (_, Kind::Numbers(start)) => div()
                        .text_right()
                        .pr(px(6.))
                        .child(format!("{}.", start + ix as u64))
                        .into_any_element(),
                    _ => div().pl(px(6.)).child(bullet).into_any_element(),
                };
                div()
                    .flex()
                    .child(div().w(px(22.)).flex_none().child(marker))
                    .child(div().flex_1().min_w_0().child(self.flow(item, window, cx)))
            })
            .collect();
        self.depth -= 1;
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(items)
            .into_any_element()
    }

    fn fence(
        &mut self,
        node: &Node,
        lang: Option<SharedString>,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let style = &*self.on.style;
        let [Piece::Text(code)] = node.flow() else {
            return div().into_any_element();
        };
        let id = self.id("code");
        if let Some(fence) = &style.fence {
            return fence(
                Fence {
                    id,
                    lang,
                    code: code.text.clone(),
                },
                window,
                cx,
            );
        }
        let colours = style
            .code
            .clone()
            .and_then(|colour| colour(lang.as_ref(), &code.text, cx));
        let title = div()
            .h(px(23.))
            .px(px(10.))
            .flex()
            .items_center()
            .border_b_1()
            .border_color(style.line_soft)
            .text_size(px(10.))
            .text_color(style.ink_faint)
            .child(div().flex_1().min_w_0().truncate().children(lang))
            .child(self.copy(code.text.clone()));
        let body = div()
            .id(id)
            .max_h(px(420.))
            .overflow_scroll()
            .px(px(12.))
            .py(px(9.))
            .font(style.mono.clone())
            .text_size(style.code_size)
            .line_height(relative(1.55))
            .whitespace_nowrap()
            .text_color(style.ink_dim)
            .child(self.code(code, colours));
        let frame = div()
            .rounded(style.radius)
            .border_1()
            .border_color(style.line_soft);
        frame
            .bg(style.code_ground)
            .child(title)
            .child(body)
            .into_any_element()
    }

    fn copy(&mut self, text: SharedString) -> Stateful<Div> {
        let ink = self.on.style.ink;
        div()
            .id(self.id("copy"))
            .cursor_pointer()
            .hover(move |copy| copy.text_color(ink))
            .child("Copy")
            .on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()))
            })
    }

    /// A table as rows of cells. A column is as wide as its longest text,
    /// shared out when they do not all fit, so long cells wrap and every row lines up.
    fn table(&mut self, node: &Node, cx: &mut App) -> AnyElement {
        let style = &*self.on.style;
        let Kind::Table(table) = &node.kind else {
            return div().into_any_element();
        };
        let cell = |cell: &Node| match cell.flow() {
            [Piece::Text(rich)] => Some(rich.clone()),
            _ => None,
        };
        let head = table.head.iter().map(|row| (row, true));
        let rows: Vec<(Vec<Option<Rich>>, bool)> = head
            .chain(node.children.iter().map(|row| (&row.children, false)))
            .map(|(row, head)| (row.iter().map(cell).collect(), head))
            .collect();
        let columns = rows
            .iter()
            .map(|(row, _)| row.len())
            .chain([table.align.len()])
            .max();
        let text =
            |row: &[Option<Rich>], x: usize| row.get(x).cloned().flatten().map(|rich| rich.text);
        let longest: Vec<f32> = (0..columns.unwrap_or(0))
            .map(|x| {
                rows.iter()
                    .filter_map(|(row, _)| text(row, x))
                    .map(|text| text.chars().count())
                    .max()
                    .unwrap_or(0) as f32
            })
            .collect();
        let copy = self
            .copy(node.table_source().unwrap_or_default().into())
            .absolute()
            .top(px(4.))
            .right(px(8.));
        let copy = copy
            .text_size(px(10.))
            .text_color(style.ink_faint)
            .invisible();
        let rows: Vec<_> = rows
            .into_iter()
            .enumerate()
            .map(|(y, (row, head))| {
                let cells: Vec<_> = longest
                    .iter()
                    .enumerate()
                    .map(|(x, chars)| {
                        let content = match row.get(x).cloned().flatten() {
                            Some(rich) if head => rich.text.to_uppercase().into_any_element(),
                            Some(rich) => self.rich(&rich, cx),
                            None => div().into_any_element(),
                        };
                        div()
                            .flex_basis(style.table_size * (chars.min(8.) * 0.6) + px(20.))
                            .flex_grow(chars.max(1.))
                            .min_w_0()
                            .px(px(10.))
                            .py(px(5.))
                            .border_color(style.line)
                            .when(x + 1 < longest.len(), Div::border_r_1)
                            .text_align(table.align.get(x).copied().unwrap_or_default())
                            .when(head || x == 0, |cell| cell.font_weight(FontWeight::MEDIUM))
                            .child(content)
                    })
                    .collect();
                let row = div()
                    .flex()
                    .border_color(style.line)
                    .when(y > 0, Div::border_t_1);
                row.when(head, |row| {
                    let size = style.table_size - px(1.5);
                    spaced(row.bg(style.table_head))
                        .text_size(size)
                        .text_color(style.ink_dim)
                })
                .children(cells)
            })
            .collect();
        div()
            .id(self.id("table"))
            .group("table")
            .relative()
            .rounded(style.radius)
            .border_1()
            .border_color(style.line)
            .bg(style.table_ground)
            .text_size(style.table_size)
            .children(rows)
            .child(copy.group_hover("table", |copy| copy.visible()))
            .into_any_element()
    }

    fn details(
        &mut self,
        node: &Node,
        summary: &SharedString,
        written_open: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.folds += 1;
        let key = (self.block, self.folds);
        let turned = self.on.turned.clone();
        let open = written_open != turned.borrow().contains(&key);
        let block = self.on.blocks[self.block].clone();
        let summary = if summary.is_empty() {
            "Details".into()
        } else {
            summary.clone()
        };
        let toggle = div()
            .id(self.id("details"))
            .flex()
            .gap(px(6.))
            .cursor_pointer()
            .text_color(self.on.style.ink_dim)
            .child(if open { "▾" } else { "▸" })
            .child(summary)
            .on_click(move |_, window, _| {
                let mut turned = turned.borrow_mut();
                if !turned.remove(&key) {
                    turned.insert(key);
                }
                block.size.set(None);
                window.refresh();
            });
        let content = open.then(|| div().pl(px(16.)).child(self.flow(node, window, cx)));
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(toggle)
            .children(content)
            .into_any_element()
    }
}

/// Each run's look: its marks, and the selection's ground over the part selected.
fn highlights(
    rich: &Rich,
    selected: Option<Range<usize>>,
    found: &[(Range<usize>, bool)],
    style: &Style,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let ranges = rich
        .marks
        .iter()
        .map(|(range, _)| range.clone())
        .chain(selected.clone())
        .chain(found.iter().map(|(range, _)| range.clone()));
    let mut edges: Vec<usize> = ranges.flat_map(|range| [range.start, range.end]).collect();
    edges.sort_unstable();
    edges.dedup();
    let inside = |outer: &Range<usize>, inner: &Range<usize>| {
        outer.start <= inner.start && inner.end <= outer.end
    };
    edges
        .windows(2)
        .filter_map(|edge| {
            let run = edge[0]..edge[1];
            let marks = rich
                .marks
                .iter()
                .find(|(range, _)| inside(range, &run))
                .map(|(_, marks)| *marks);
            let chosen = selected
                .as_ref()
                .is_some_and(|selected| inside(selected, &run));
            let hit = found.iter().find(|(range, _)| inside(range, &run));
            let mut look = match marks {
                Some(marks) => highlight(marks, style),
                None if chosen || hit.is_some() => HighlightStyle::default(),
                None => return None,
            };
            if let Some((_, current)) = hit {
                look.background_color = Some(if *current {
                    style.found_current
                } else {
                    style.found
                });
            }
            if chosen {
                look.background_color = Some(style.selection);
            }
            Some((run, look))
        })
        .collect()
}

fn highlight(marks: Marks, style: &Style) -> HighlightStyle {
    let mut highlight = HighlightStyle::default();
    if marks.has(Marks::STRONG) {
        highlight.font_weight = Some(FontWeight::SEMIBOLD);
    }
    if marks.has(Marks::EMPHASIS) {
        highlight.font_style = Some(FontStyle::Italic);
    }
    if marks.has(Marks::STRIKE) {
        highlight.strikethrough = Some(StrikethroughStyle {
            thickness: px(1.),
            color: None,
        });
    }
    if marks.has(Marks::CODE) {
        highlight.color = Some(style.code_ink);
        highlight.background_color = Some(style.mark_ground);
        #[cfg(feature = "fork")]
        {
            highlight.font_scale = Some(gpui::FontScale(0.88));
        }
    }
    if marks.has(Marks::LINK) {
        highlight.color = Some(style.accent);
    }
    highlight
}

#[cfg(feature = "fork")]
fn spaced(row: Div) -> Div {
    row.letter_spacing(0.06)
}

#[cfg(not(feature = "fork"))]
fn spaced(row: Div) -> Div {
    row
}

fn tasks_in(node: &Node) -> usize {
    let own = usize::from(matches!(node.kind, Kind::Item(Some(_))));
    own + node.children.iter().map(tasks_in).sum::<usize>()
}

/// A task's words, without those of any list under it.
fn task_words(item: &Node) -> String {
    fn gather(node: &Node, into: &mut String) {
        if matches!(node.kind, Kind::Bullets | Kind::Numbers(_)) {
            return;
        }
        into.push_str(&node.text);
        for child in &node.children {
            gather(child, into);
        }
    }
    let mut words = String::new();
    for child in &item.children {
        gather(child, &mut words);
    }
    words.trim().to_owned()
}

fn task_box(checked: bool, style: &Style) -> Div {
    let tick = div()
        .mt(px(5.))
        .size(px(13.))
        .rounded(px(3.))
        .border_1()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(10.))
        .line_height(px(10.));
    if checked {
        tick.bg(style.accent)
            .border_color(style.accent)
            .text_color(gpui::white())
            .child("✓")
    } else {
        tick.border_color(style.ink_faint)
    }
}
