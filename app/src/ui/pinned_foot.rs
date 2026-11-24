//! A column in a scrolling area whose last part stays in view at the area's bottom.

use gpui_kit::{
    fill, point, px, size, AlignItems, AnyElement, App, Bounds, ContentMask, Display, Element,
    ElementId, FlexDirection, GlobalElementId, InspectorElementId, IntoElement, JustifyContent,
    LayoutId, Pixels, Style, Window,
};

use crate::theme::Tokens;

/// The least of the scrolled part that shows over a pinned foot.
const HEAD_SHOWN: Pixels = px(120.);

/// `head` then `foot` down a scrolling area, as tall as the area at least, with the foot at its
/// bottom. A head too tall for the rest scrolls under the foot's top edge; a foot that leaves
/// the head too little room scrolls along with it.
pub(crate) fn pinned_foot(head: impl IntoElement, foot: impl IntoElement) -> PinnedFoot {
    PinnedFoot {
        head: head.into_any_element(),
        foot: foot.into_any_element(),
    }
}

pub(crate) struct PinnedFoot {
    head: AnyElement,
    foot: AnyElement,
}

impl IntoElement for PinnedFoot {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for PinnedFoot {
    type RequestLayoutState = LayoutId;
    /// Where the head is cut off, once the foot covers part of it.
    type PrepaintState = Option<Bounds<Pixels>>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, LayoutId) {
        let head = self.head.request_layout(window, cx);
        let foot = self.foot.request_layout(window, cx);
        let style = Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            justify_content: Some(JustifyContent::SpaceBetween),
            align_items: Some(AlignItems::Stretch),
            flex_grow: 1.,
            flex_shrink: 0.,
            ..Style::default()
        };
        (window.request_layout(style, [head, foot], cx), foot)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        foot: &mut LayoutId,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let view = window.content_mask().bounds;
        let foot = window.layout_bounds(*foot);
        let pins = foot.size.height + HEAD_SHOWN <= view.size.height;
        let lift = match pins {
            true => (view.bottom() - foot.bottom()).min(px(0.)),
            false => px(0.),
        };
        let cut = (lift < px(0.)).then(|| {
            let height = foot.top() + lift - view.top();
            Bounds::new(view.origin, size(view.size.width, height))
        });
        window.with_content_mask(cut.map(|bounds| ContentMask { bounds }), |window| {
            self.head.prepaint(window, cx)
        });
        window.with_element_offset(point(px(0.), lift), |window| self.foot.prepaint(window, cx));
        cut
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut LayoutId,
        cut: &mut Option<Bounds<Pixels>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_content_mask(cut.map(|bounds| ContentMask { bounds }), |window| {
            self.head.paint(window, cx)
        });
        if let Some(cut) = cut {
            let at = point(bounds.left(), cut.bottom());
            let edge = Bounds::new(at, size(bounds.size.width, px(1.)));
            window.paint_quad(fill(edge, Tokens::get(cx).line));
        }
        self.foot.paint(window, cx);
    }
}
