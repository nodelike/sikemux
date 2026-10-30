//! Where a tab's view sits in the window: its frame, the holes cut for overlays, and the swipes
//! it hands to the stage.

use super::*;

/// Puts the tab's view at `frame`, given top-down in the window's content
/// view. With `page`, the page lays out at that size and the view draws it
/// scaled into `frame`: its bounds take the page's size, so AppKit scales the
/// drawing and maps every event point back into page pixels on its own.
pub fn place(webview: &WKWebView, frame: NSRect, page: Option<NSSize>) {
    let webview = webview.retain();
    let view: &NSView = &webview;
    // SAFETY: main thread (where `Tab` runs every call), and `webview` is retained.
    let Some(parent) = (unsafe { view.superview() }) else {
        return;
    };
    let origin = if parent.isFlipped() {
        frame.origin
    } else {
        NSPoint::new(
            frame.origin.x,
            parent.frame().size.height - frame.origin.y - frame.size.height,
        )
    };
    let page_size = page.unwrap_or(frame.size);
    if view.frame().size != frame.size || view.bounds().size != page_size {
        // WebKit lays the page out at whatever size its own `setFrameSize:` is
        // given, so it hears the page's size, and the view then takes the
        // smaller frame through NSView's method, which WebKit never sees.
        view.setFrameSize(page_size);
        if page.is_some() {
            // SAFETY: `view` is a live NSView on the main thread, and NSView's
            // `setFrameSize:` takes one NSSize and returns nothing.
            let _: () =
                unsafe { msg_send![super(view, NSView::class()), setFrameSize: frame.size] };
        }
        view.setBoundsSize(page_size);
        // WebKit's own content view fills the view by resizing with it, and a
        // change of bounds alone resizes nothing.
        let whole = view.bounds();
        let fills = NSAutoresizingMaskOptions::ViewWidthSizable
            | NSAutoresizingMaskOptions::ViewHeightSizable;
        for child in view.subviews() {
            if child.autoresizingMask().contains(fills) {
                child.setFrame(whole);
            }
        }
    }
    view.setFrameOrigin(origin);
}

/// Draw the page less `clip_left` and `clip_right` off its sides and less the
/// `holes`, all in the page's own top-down pixels. A native view is not cut
/// off by the DOM around it, so a swipe would carry the page over the rails
/// and it would cover any toast.
pub fn clip(webview: &WKWebView, clip_left: f64, clip_right: f64, holes: Vec<(NSRect, f64)>) {
    let webview = webview.retain();
    let view: &NSView = &webview;
    // SAFETY: every NSView answers `layer` with a CALayer or nil, and the result is retained.
    let Some(layer): Option<Retained<CALayer>> = (unsafe { msg_send![view, layer] }) else {
        return;
    };
    let whole = view.bounds();
    let visible = NSRect::new(
        NSPoint::new(whole.origin.x + clip_left, whole.origin.y),
        NSSize::new(
            (whole.size.width - clip_left - clip_right).max(0.0),
            whole.size.height,
        ),
    );
    let holes: Vec<(NSRect, NSRect, f64)> = holes
        .into_iter()
        .filter_map(|(whole_hole, radius)| {
            let hole = intersection(whole_hole, visible)?;
            Some((
                hole,
                whole_hole,
                radius
                    .min(hole.size.width / 2.0)
                    .min(hole.size.height / 2.0),
            ))
        })
        .collect();
    HOLES.with(|all| {
        let mut all = all.borrow_mut();
        if holes.is_empty() {
            all.remove(&view_key(&webview));
        } else {
            all.insert(
                view_key(&webview),
                holes.iter().map(|(hole, _, _)| *hole).collect(),
            );
        }
    });
    if !holes.is_empty() {
        let _ = pass_clicks_through_holes(view);
    }
    let flip = |rect: NSRect| {
        if layer.isGeometryFlipped() {
            rect
        } else {
            NSRect::new(
                NSPoint::new(
                    rect.origin.x,
                    whole.size.height - rect.origin.y - rect.size.height,
                ),
                rect.size,
            )
        }
    };
    let key = view_key(&webview);
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    MASKS.with(|masks| {
        let mut masks = masks.borrow_mut();
        if visible == whole && holes.is_empty() {
            masks.remove(&key);
            // SAFETY: main thread, and `layer` is retained for the whole function.
            unsafe { layer.setMask(None) };
            return;
        }
        /* A swipe clips the page on every frame it is partly off stage. The mask
        is kept and moved rather than rebuilt, and a plain rectangle is a layer's
        frame rather than a path that has to be drawn again. */
        let mask: &CALayer = if holes.is_empty() {
            if !matches!(masks.get(&key), Some(PageMask::Rect(_))) {
                let rect = CALayer::new();
                rect.setBackgroundColor(Some(&CGColor::new_generic_gray(0.0, 1.0)));
                masks.insert(key, PageMask::Rect(rect));
            }
            let Some(PageMask::Rect(rect)) = masks.get(&key) else {
                return;
            };
            rect.setFrame(flip(visible));
            rect
        } else {
            let path = CGMutablePath::new();
            // SAFETY: a null transform means none, and `path` is a fresh path only we hold.
            unsafe {
                CGMutablePath::add_rect(Some(&path), std::ptr::null(), flip(visible));
                for (hole, whole_hole, radius) in &holes {
                    add_hole(&path, flip(*hole), flip(*whole_hole), *radius);
                }
            }
            if !matches!(masks.get(&key), Some(PageMask::Shape(_))) {
                let shape = CAShapeLayer::new();
                // SAFETY: an immutable constant string QuartzCore sets up when it loads.
                shape.setFillRule(unsafe { kCAFillRuleEvenOdd });
                masks.insert(key, PageMask::Shape(shape));
            }
            let Some(PageMask::Shape(shape)) = masks.get(&key) else {
                return;
            };
            shape.setFrame(layer.bounds());
            shape.setPath(Some(&path));
            shape
        };
        let current = layer.mask();
        if current.as_deref().map(|layer| layer as *const CALayer) != Some(mask as *const CALayer) {
            // SAFETY: main thread, and the layer retains `mask` from here on.
            unsafe { layer.setMask(Some(mask)) };
        }
    });
    CATransaction::commit();
}

/// Round the page's two lower corners by `radius`, so it fits inside a pane's rounded edge.
pub fn round_bottom(webview: &WKWebView, radius: f64) {
    let view: &NSView = webview;
    // SAFETY: every NSView answers `layer` with a CALayer or nil, and the result is retained.
    let Some(layer): Option<Retained<CALayer>> = (unsafe { msg_send![view, layer] }) else {
        return;
    };
    let bottom = if layer.isGeometryFlipped() {
        CACornerMask::LayerMinXMaxYCorner | CACornerMask::LayerMaxXMaxYCorner
    } else {
        CACornerMask::LayerMinXMinYCorner | CACornerMask::LayerMaxXMinYCorner
    };
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    layer.setCornerRadius(radius);
    layer.setMaskedCorners(bottom);
    layer.setMasksToBounds(radius > 0.0);
    CATransaction::commit();
}

/// Fade the whole page, so it goes with the pane it sits in when that fades.
pub fn fade(webview: &WKWebView, opacity: f64) {
    let webview = webview.retain();
    let view: &NSView = &webview;
    // SAFETY: `setAlphaValue:` is an NSView method taking a CGFloat.
    let _: () = unsafe { msg_send![view, setAlphaValue: opacity] };
}

/// Lay a black shade of `alpha` over the page, or take it off at zero, so an
/// app panel floating on the page stands apart from it. The shade lives in the
/// page's own layer, so the mask `clip` sets cuts it where the panel is.
pub fn dim(webview: &WKWebView, alpha: f64) {
    let webview = webview.retain();
    let view: &NSView = &webview;
    // SAFETY: every NSView answers `layer` with a CALayer or nil, and the result is retained.
    let Some(layer): Option<Retained<CALayer>> = (unsafe { msg_send![view, layer] }) else {
        return;
    };
    let key = view_key(&webview);
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    SHADES.with(|shades| {
        let mut shades = shades.borrow_mut();
        if alpha <= 0.0 {
            if let Some(shade) = shades.remove(&key) {
                shade.removeFromSuperlayer();
            }
            return;
        }
        let shade = shades.entry(key).or_insert_with(|| {
            let shade = CALayer::new();
            // Above whatever layers WebKit draws the page into.
            shade.setZPosition(1.0e6);
            layer.addSublayer(&shade);
            shade
        });
        shade.setFrame(layer.bounds());
        shade.setBackgroundColor(Some(&CGColor::new_generic_gray(0.0, alpha)));
    });
    CATransaction::commit();
}

pub(super) fn view_key(view: &NSView) -> usize {
    view as *const NSView as usize
}

/// Round only the corners `hole` shares with `whole_hole`. Where the page's edge
/// cuts a menu short, the cut stays square instead of curving into the menu.
pub(super) fn add_hole(path: &CGMutablePath, hole: NSRect, whole_hole: NSRect, radius: f64) {
    let same = |a: f64, b: f64| (a - b).abs() < 0.5;
    let (min_x, min_y) = (hole.origin.x, hole.origin.y);
    let (max_x, max_y) = (min_x + hole.size.width, min_y + hole.size.height);
    let left = same(min_x, whole_hole.origin.x);
    let right = same(max_x, whole_hole.origin.x + whole_hole.size.width);
    let low = same(min_y, whole_hole.origin.y);
    let high = same(max_y, whole_hole.origin.y + whole_hole.size.height);
    let corner = |x: bool, y: bool| if x && y { radius } else { 0.0 };
    let path = Some(path);
    let none = std::ptr::null();
    // SAFETY: a null transform means none.
    unsafe {
        CGMutablePath::move_to_point(path, none, (min_x + max_x) / 2.0, min_y);
        CGMutablePath::add_arc_to_point(path, none, max_x, min_y, max_x, max_y, corner(right, low));
        CGMutablePath::add_arc_to_point(
            path,
            none,
            max_x,
            max_y,
            min_x,
            max_y,
            corner(right, high),
        );
        CGMutablePath::add_arc_to_point(path, none, min_x, max_y, min_x, min_y, corner(left, high));
        CGMutablePath::add_arc_to_point(path, none, min_x, min_y, max_x, min_y, corner(left, low));
    }
    CGMutablePath::close_subpath(path);
}

pub(super) fn intersection(a: NSRect, b: NSRect) -> Option<NSRect> {
    let left = a.origin.x.max(b.origin.x);
    let top = a.origin.y.max(b.origin.y);
    let right = (a.origin.x + a.size.width).min(b.origin.x + b.size.width);
    let bottom = (a.origin.y + a.size.height).min(b.origin.y + b.size.height);
    (right > left && bottom > top).then(|| {
        NSRect::new(
            NSPoint::new(left, top),
            NSSize::new(right - left, bottom - top),
        )
    })
}

/* A mask only changes what the page draws; clicks over a hole still land on the
page. So the page's class learns to pass a point in a hole on to the app below. */
pub(super) fn pass_clicks_through_holes(view: &NSView) -> Option<()> {
    if PAGE_HIT_TEST.with(|cell| cell.get()).is_some() {
        return Some(());
    }
    let class = view.class();
    let selector = sel!(hitTest:);
    let inherited = class.instance_method(selector)?;
    // SAFETY: `inherited` is a real method of the view's class, so its type string lives
    // as long as the class does.
    let types = unsafe { objc2::ffi::method_getTypeEncoding(inherited) };
    let hit_test: HitTest = hit_test_outside_holes;
    // SAFETY: `hit_test_outside_holes` has `hitTest:`'s exact signature and reuses its
    // type string. `class_addMethod` only adds to the class, never replaces a method.
    let added = unsafe {
        objc2::ffi::class_addMethod(
            (class as *const objc2::runtime::AnyClass).cast_mut(),
            selector,
            std::mem::transmute::<HitTest, Imp>(hit_test),
            types,
        )
    };
    added
        .as_bool()
        .then(|| PAGE_HIT_TEST.with(|cell| cell.set(Some(inherited.implementation()))))
}

/* A two-finger swipe that starts over a page goes to the page, so the stage
never hears it and cannot move. WebKit reports each scroll the page did not
use, which is what its own back and forward swipe is built on. A sideways
swipe the page's history cannot take is handed to the app's own view
instead, where it moves the stage as it would anywhere else. */
pub(super) fn hand_unused_swipes_to_the_stage(view: &NSView) -> Option<()> {
    if PAGE_UNUSED_GESTURE.with(|cell| cell.get()).is_some() {
        return Some(());
    }
    let class = view.class();
    let selector = sel!(_gestureEventWasNotHandledByWebCore:);
    let inherited = class.instance_method(selector)?;
    // SAFETY: `inherited` is a real method of the view's class, so its type string lives
    // as long as the class does.
    let types = unsafe { objc2::ffi::method_getTypeEncoding(inherited) };
    let unused: UnusedGesture = gesture_unused_by_page;
    // SAFETY: `gesture_unused_by_page` has the method's exact signature and reuses its
    // type string. `class_addMethod` only adds to the class, never replaces a method.
    let added = unsafe {
        objc2::ffi::class_addMethod(
            (class as *const objc2::runtime::AnyClass).cast_mut(),
            selector,
            std::mem::transmute::<UnusedGesture, Imp>(unused),
            types,
        )
    };
    added
        .as_bool()
        .then(|| PAGE_UNUSED_GESTURE.with(|cell| cell.set(Some(inherited.implementation()))))
}

pub(super) type UnusedGesture = unsafe extern "C-unwind" fn(&NSView, Sel, *mut NSEvent);

// SAFETY: only WebKit calls this, as `_gestureEventWasNotHandledByWebCore:` on a
// live view on the main thread, with an event or nil.
unsafe extern "C-unwind" fn gesture_unused_by_page(
    view: &NSView,
    selector: Sel,
    event: *mut NSEvent,
) {
    // SAFETY: WebKit passes a live event or nil for the length of the call.
    let stage = unsafe { event.as_ref() }.and_then(|event| stage_for_swipe(view, event));
    if let Some(stage) = stage {
        // SAFETY: `scrollWheel:` takes one event and returns nothing; the stage's view
        // is live and on the main thread.
        let _: () = unsafe { msg_send![&*stage, scrollWheel: event] };
        return;
    }
    let Some(inherited) = PAGE_UNUSED_GESTURE.with(|cell| cell.get()) else {
        return;
    };
    // SAFETY: `inherited` is the implementation this function stands in front of.
    let inherited = unsafe { std::mem::transmute::<Imp, UnusedGesture>(inherited) };
    // SAFETY: passes WebKit's own arguments straight through.
    unsafe { inherited(view, selector, event) }
}

/// The app's own view, when this scroll is part of a sideways swipe the page
/// had no use for and its history cannot take either.
pub(super) fn stage_for_swipe(view: &NSView, event: &NSEvent) -> Option<Retained<NSView>> {
    if event.r#type() != NSEventType::ScrollWheel {
        return None;
    }
    let tab = TABS.with(|tabs| {
        tabs.borrow()
            .values()
            .find(|tab| view_key(&tab.webview) == view_key(view))
            .map(|tab| tab.webview.clone())
    })?;
    let phase = event.phase();
    if phase.intersects(NSEventPhase::MayBegin | NSEventPhase::Began) {
        SWIPE_TO_STAGE.with(|cell| cell.set(None));
    }
    let to_stage = match SWIPE_TO_STAGE.with(|cell| cell.get()) {
        Some(decided) => decided,
        None => {
            let (across, down) = (event.scrollingDeltaX(), event.scrollingDeltaY());
            if across == 0.0 || across.abs() <= down.abs() {
                return None;
            }
            // Fingers moving right ask for the page before; left for the one after.
            // SAFETY: main thread, and `tab` is retained.
            let history = unsafe {
                if across > 0.0 {
                    tab.canGoBack()
                } else {
                    tab.canGoForward()
                }
            };
            SWIPE_TO_STAGE.with(|cell| cell.set(Some(!history)));
            !history
        }
    };
    let momentum = event.momentumPhase();
    if momentum.contains(NSEventPhase::Ended)
        || (momentum.is_empty() && phase.intersects(NSEventPhase::Ended | NSEventPhase::Cancelled))
    {
        SWIPE_TO_STAGE.with(|cell| cell.set(None));
    }
    if !to_stage {
        return None;
    }
    // SAFETY: main thread, and `view` is alive for this call.
    let parent = unsafe { view.superview() }?;
    let pages: Vec<usize> = TABS.with(|tabs| {
        tabs.borrow()
            .values()
            .map(|tab| view_key(&tab.webview))
            .collect()
    });
    let web_view_class = objc2::runtime::AnyClass::get(c"WKWebView")?;
    let app_page = parent.subviews().iter().find(|sibling| {
        !pages.contains(&view_key(sibling)) && sibling.isKindOfClass(web_view_class)
    });
    Some(app_page.unwrap_or(parent))
}

pub(super) type HitTest = unsafe extern "C-unwind" fn(&NSView, Sel, NSPoint) -> *mut NSView;

// SAFETY: only AppKit calls this, as the `hitTest:` method installed above, with a
// live view on the main thread.
unsafe extern "C-unwind" fn hit_test_outside_holes(
    view: &NSView,
    selector: Sel,
    point: NSPoint,
) -> *mut NSView {
    let holes = HOLES.with(|all| all.borrow().get(&view_key(view)).cloned());
    if let Some(holes) = holes {
        // SAFETY: main thread, and `view` is alive for this call.
        let superview = unsafe { view.superview() };
        let local = view.convertPoint_fromView(point, superview.as_deref());
        let local = if view.isFlipped() {
            local
        } else {
            NSPoint::new(local.x, view.bounds().size.height - local.y)
        };
        let inside = |hole: &NSRect| {
            local.x >= hole.origin.x
                && local.x < hole.origin.x + hole.size.width
                && local.y >= hole.origin.y
                && local.y < hole.origin.y + hole.size.height
        };
        if holes.iter().any(inside) {
            return std::ptr::null_mut();
        }
    }
    let Some(inherited) = PAGE_HIT_TEST.with(|cell| cell.get()) else {
        return std::ptr::null_mut();
    };
    // SAFETY: `inherited` is the `hitTest:` implementation this function stands in
    // front of, so it has the same signature.
    let inherited = unsafe { std::mem::transmute::<Imp, HitTest>(inherited) };
    // SAFETY: passes AppKit's own arguments straight to the original `hitTest:`.
    unsafe { inherited(view, selector, point) }
}
