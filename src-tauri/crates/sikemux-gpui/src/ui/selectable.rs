//! Text outside the transcript the person can select and copy, drawn in the colours its runs
//! were given, as the Mac lets them take a question's words.

use std::ops::Range;

use gpui::{
    App, BorderStyle, Bounds, Corners, Edges, Element, ElementId, GlobalElementId, HighlightStyle,
    Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId, PaintQuad, Pixels,
    SharedString, StyledText, Window,
};
use gpui_kit::base::{
    TextSelection, TextSelectionHandle, TextSelectionRegistration, TextSelectionRun,
};

use crate::theme::Tokens;

/// `text`, with each range in `runs` drawn its own way, selectable as one piece under `id`.
pub fn selectable(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    runs: impl IntoIterator<Item = (Range<usize>, HighlightStyle)>,
) -> Selectable {
    let text = text.into();
    Selectable {
        id: id.into(),
        styled: StyledText::new(text.clone()).with_highlights(runs),
        text,
    }
}

pub struct Selectable {
    id: ElementId,
    text: SharedString,
    styled: StyledText,
}

impl IntoElement for Selectable {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Selectable {
    type RequestLayoutState = TextSelectionHandle;
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, TextSelectionHandle) {
        let handle = window.with_element_state(
            global_id.expect("selectable text has an id"),
            |kept: Option<TextSelectionHandle>, _| {
                let handle =
                    kept.unwrap_or_else(|| TextSelectionHandle::new(self.text.clone(), cx));
                (handle.clone(), handle)
            },
        );
        handle.set_fallback_copy_text(self.text.clone(), cx);
        let (layout, ()) = self
            .styled
            .request_layout(global_id, inspector_id, window, cx);
        (layout, handle)
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        handle: &mut TextSelectionHandle,
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        self.styled
            .prepaint(global_id, inspector_id, bounds, &mut (), window, cx);
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let registration = TextSelectionRegistration::new(hitbox.clone(), bounds)
            .with_text_bounds(vec![bounds])
            .with_rendered_element(handle, window, cx);
        handle.register(registration, window, cx);
        hitbox
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        handle: &mut TextSelectionHandle,
        _: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = self.styled.layout().clone();
        let before = TextSelection::selected_text(window, cx);
        let projection = handle.update_runs(
            &[TextSelectionRun::new(
                self.text.clone(),
                layout.clone(),
                bounds,
            )],
            cx,
        );
        if before != TextSelection::selected_text(window, cx) {
            window.refresh();
        }
        let accent = Tokens::get(cx).accent;
        let color = Hsla { a: 0.3, ..accent };
        for range in projection.ranges().iter().flatten() {
            let (Some(start), Some(end)) = (
                layout.position_for_index(range.start),
                layout.position_for_index(range.end),
            ) else {
                continue;
            };
            let (line, edge) = (layout.line_height(), layout.bounds());
            let mut quads = vec![Bounds::from_corners(
                start,
                gpui::point(
                    if start.y == end.y {
                        end.x
                    } else {
                        edge.right()
                    },
                    start.y + line,
                ),
            )];
            if end.y > start.y {
                if end.y > start.y + line {
                    quads.push(Bounds::from_corners(
                        gpui::point(edge.left(), start.y + line),
                        gpui::point(edge.right(), end.y),
                    ));
                }
                quads.push(Bounds::from_corners(
                    gpui::point(edge.left(), end.y),
                    gpui::point(end.x, end.y + line),
                ));
            }
            for quad in quads {
                window.paint_quad(PaintQuad {
                    bounds: quad,
                    background: color.into(),
                    corner_radii: Corners::default(),
                    border_widths: Edges::default(),
                    border_color: gpui::transparent_black(),
                    border_style: BorderStyle::default(),
                });
            }
        }
        self.styled.paint(
            global_id,
            inspector_id,
            bounds,
            &mut (),
            &mut (),
            window,
            cx,
        );
    }
}
