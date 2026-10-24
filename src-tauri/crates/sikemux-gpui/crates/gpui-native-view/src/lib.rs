//! Shows a native view, such as a web page or a PDF, in a GPUI element's place.
//!
//! - [`Surface`] is what the element moves: anything that can be shown over a
//!   frame of the window and put out of sight again.
//! - [`NativeSurface`] shares one surface between the frames that draw it.
//! - [`native_view`] is the element. It lays out like any other, hands the
//!   surface its frame whenever that changes, and puts it out of sight once
//!   the element is no longer drawn.
//! - [`document::Document`] is a surface for a file macOS draws itself: PDFs in
//!   PDFKit's view, office and iWork files in Quick Look's.
//!
//! Native views sit above everything GPUI draws, so a window that holds them
//! should call `Window::enable_scene_overlay` as it opens: popovers, menus and
//! anything else drawn deferred then show above the views, and take the
//! clicks where they sit.

#[cfg(target_os = "macos")]
pub mod document;
#[cfg(target_os = "macos")]
mod macos;

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, Bounds, Element, ElementId, FocusHandle, GlobalElementId, HitboxBehavior,
    InspectorElementId, IntoElement, LayoutId, Pixels, Refineable, Style, StyleRefinement, Styled,
    Window,
};

#[cfg(target_os = "macos")]
pub use macos::{window_view, OwnedView};
#[cfg(target_os = "macos")]
pub use objc2::rc::Retained;
#[cfg(target_os = "macos")]
pub use objc2_app_kit::NSView;

/// Something native shown in an element's place.
pub trait Surface: 'static {
    /// Shows the surface over `frame`, or puts it out of sight on `None`.
    fn place(&self, frame: Option<Frame>);

    /// The view that holds the keyboard while the element's focus handle is focused.
    #[cfg(target_os = "macos")]
    fn responder(&self) -> Option<Retained<NSView>> {
        None
    }
}

/// Where an element sits, in window points from the top left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub bounds: Bounds<Pixels>,
    /// The part of `bounds` its ancestors let show, as a scrolled list's edge.
    pub visible: Bounds<Pixels>,
}

/// A surface and which element last placed it.
#[derive(Clone)]
pub struct NativeSurface(Rc<Shared>);

struct Shared {
    surface: Box<dyn Surface>,
    placed_by: Cell<u64>,
    placed: Cell<Option<Frame>>,
}

impl NativeSurface {
    pub fn new(surface: impl Surface) -> Self {
        Self(Rc::new(Shared {
            surface: Box::new(surface),
            placed_by: Cell::new(0),
            placed: Cell::new(None),
        }))
    }

    pub fn surface(&self) -> &dyn Surface {
        &*self.0.surface
    }

    /// Where the surface was last placed, or none while it is hidden.
    pub fn placed(&self) -> Option<Frame> {
        self.0.placed.get()
    }

    fn place(&self, by: u64, frame: Option<Frame>) {
        self.0.placed_by.set(by);
        if self.0.placed.replace(frame) != frame {
            self.0.surface.place(frame);
        }
    }
}

/// Shows `surface` where this element is laid out. Style it as any other element.
pub fn native_view(id: impl Into<ElementId>, surface: &NativeSurface) -> NativeView {
    NativeView {
        id: id.into(),
        surface: surface.clone(),
        focus: None,
        style: StyleRefinement::default(),
    }
}

pub struct NativeView {
    id: ElementId,
    surface: NativeSurface,
    focus: Option<FocusHandle>,
    style: StyleRefinement,
}

impl NativeView {
    /// Keeps the native keyboard focus and `focus` together: focusing either
    /// one focuses the other, and the keyboard returns to GPUI when `focus` loses it.
    pub fn track_focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }
}

impl Styled for NativeView {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl IntoElement for NativeView {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for NativeView {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
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
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.refine(&self.style);
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(focus) = &self.focus {
            window.set_focus_handle(focus, cx);
        }
        let visible = bounds.intersect(&window.content_mask().bounds);
        window.insert_hitbox(visible, HitboxBehavior::BlockMouseExceptScroll);
        let frame = (!visible.is_empty()).then_some(Frame { bounds, visible });
        let id = id.expect("a native view has an id");
        window.with_element_state(id, |placement: Option<Placement>, window| {
            let placement = placement
                .filter(|placement| placement.shows(&self.surface))
                .unwrap_or_else(|| Placement::new(&self.surface, self.focus.as_ref(), window, cx));
            self.surface.place(placement.mark, frame);
            ((), placement)
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }
}

/// An element's hold on its surface, kept from frame to frame while the element
/// is drawn. GPUI drops it with the first frame that leaves the element out.
struct Placement {
    surface: NativeSurface,
    mark: u64,
    #[cfg(target_os = "macos")]
    keyboard: Option<macos::Keyboard>,
}

thread_local! {
    static MARKS: Cell<u64> = const { Cell::new(0) };
}

impl Placement {
    fn new(
        surface: &NativeSurface,
        focus: Option<&FocusHandle>,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let mark = MARKS.with(|marks| {
            marks.set(marks.get() + 1);
            marks.get()
        });
        #[cfg(not(target_os = "macos"))]
        let _ = (focus, window, cx);
        Self {
            surface: surface.clone(),
            mark,
            #[cfg(target_os = "macos")]
            keyboard: focus.and_then(|focus| macos::Keyboard::new(surface, focus, window, cx)),
        }
    }

    fn shows(&self, surface: &NativeSurface) -> bool {
        Rc::ptr_eq(&self.surface.0, &surface.0)
    }
}

impl Drop for Placement {
    fn drop(&mut self) {
        // A view hidden while it holds the keyboard hands it to the window, not to GPUI.
        #[cfg(target_os = "macos")]
        drop(self.keyboard.take());
        if self.surface.0.placed_by.get() == self.mark {
            self.surface.place(self.mark, None);
        }
    }
}
