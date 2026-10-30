//! Mouse and keyboard input for a tab, delivered as real AppKit events. A page
//! can tell a script-dispatched event from a person's (`isTrusted`), and rich
//! editors ignore values written straight into the DOM, so the agent's clicks
//! and keys take the same path through WebKit as the person's.
//!
//! Events go straight to the tab's view rather than through the window, so a
//! tab that is off screen still receives them. The person's keyboard focus
//! never moves for the agent: a tab only takes it when the person clicks into
//! it, and a key the agent presses borrows it for that key alone, unseen by
//! either page.

use std::cell::{Cell, RefCell};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::{msg_send, sel, MainThreadMarker, Message};
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType, NSResponder, NSView, NSWindow};
use objc2_foundation::{NSObjectProtocol, NSPoint, NSProcessInfo, NSString};
use objc2_web_kit::WKWebView;

/// WebKit hands a key back to the app once the page is done with it, which
/// can take a while on a busy page.
const SENT_KEY_LIFETIME: Duration = Duration::from_secs(30);
/// How long after an agent's input WebKit may still answer it with a cursor.
const CURSOR_ANSWER_WINDOW: Duration = Duration::from_secs(3);
/// No keyboard has a key with this code.
const UNUSED_KEY_CODE: u16 = 0xFF;

static ORIGINAL_SET_CURSOR: OnceLock<Imp> = OnceLock::new();
static ORIGINAL_HIDE_CURSOR: OnceLock<Imp> = OnceLock::new();
static ORIGINAL_MAKE_FIRST_RESPONDER: OnceLock<Imp> = OnceLock::new();
static ORIGINAL_BECOME_FIRST_RESPONDER: OnceLock<Imp> = OnceLock::new();
static ORIGINAL_RESIGN_FIRST_RESPONDER: OnceLock<Imp> = OnceLock::new();

thread_local! {
    static SENT_KEYS: RefCell<Vec<SentKey>> = const { RefCell::new(Vec::new()) };
    static LAST_INPUT: Cell<LastInput> = const { Cell::new(LastInput::NONE) };
    /// The view under the person's latest click.
    static PERSON_CLICKED: RefCell<Option<Weak<NSView>>> = const { RefCell::new(None) };
    /// Set while an agent key borrows the keyboard.
    static LENDING_KEYBOARD: Cell<bool> = const { Cell::new(false) };
}

/// A key-down the agent sent. A page that leaves a key unhandled has WebKit
/// send it on through the app, where it would type into whatever the person
/// has focused or fire an app shortcut.
struct SentKey {
    event: Retained<NSEvent>,
    tab: Weak<WKWebView>,
    edit: Option<Edit>,
    at: Instant,
}

/// The editing commands a text field answers to without any page script.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edit {
    SelectAll,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mouse {
    Move,
    Down,
    Drag,
    Up,
}

/// One key as AppKit describes it, parsed from names like `Enter` or `Meta+a`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyStroke {
    pub code: u16,
    pub characters: String,
    pub unmodified: String,
    pub flags: NSEventModifierFlags,
}

fn now() -> f64 {
    NSProcessInfo::processInfo().systemUptime()
}

/// `x` and `y` are CSS pixels from the top left of the page's viewport. The
/// view's bounds are in page pixels even while it is drawn scaled, so only
/// the page's own zoom stands between the two.
pub fn mouse(
    webview: &WKWebView,
    kind: Mouse,
    x: f64,
    y: f64,
    clicks: isize,
) -> Result<(), String> {
    let webview = webview.retain();
    let window = webview.window().ok_or("the tab is not in a window")?;
    note(|last| last.agent_pointer = Some(Instant::now()));
    // SAFETY: main thread (where `Tab` runs every call), and `webview` is retained.
    let zoom = unsafe { webview.pageZoom() }.max(0.01);
    let height = webview.bounds().size.height;
    let local = if webview.isFlipped() {
        NSPoint::new(x * zoom, y * zoom)
    } else {
        NSPoint::new(x * zoom, height - y * zoom)
    };
    let location = webview.convertPoint_toView(local, None);
    let kind_code = match kind {
        Mouse::Move => NSEventType::MouseMoved,
        Mouse::Down => NSEventType::LeftMouseDown,
        Mouse::Drag => NSEventType::LeftMouseDragged,
        Mouse::Up => NSEventType::LeftMouseUp,
    };
    let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        kind_code,
        location,
        NSEventModifierFlags::empty(),
        now(),
        window.windowNumber(),
        None,
        0,
        if kind == Mouse::Move { 0 } else { clicks.max(1) },
        if kind == Mouse::Up || kind == Mouse::Move { 0.0 } else { 1.0 },
    )
    .ok_or("AppKit refused the mouse event")?;
    match kind {
        Mouse::Move => webview.mouseMoved(&event),
        Mouse::Down => webview.mouseDown(&event),
        Mouse::Drag => webview.mouseDragged(&event),
        Mouse::Up => webview.mouseUp(&event),
    }
    Ok(())
}

pub fn key(webview: &WKWebView, stroke: &KeyStroke) -> Result<(), String> {
    let webview = webview.retain();
    let window = webview.window().ok_or("the tab is not in a window")?;
    note(|last| last.agent_key = Some(Instant::now()));
    let event = |kind: NSEventType| {
        NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            kind,
            NSPoint::new(0.0, 0.0),
            stroke.flags,
            now(),
            window.windowNumber(),
            None,
            &NSString::from_str(&stroke.characters),
            &NSString::from_str(&stroke.unmodified),
            false,
            stroke.code,
        )
        .ok_or_else(|| "AppKit refused the key event".to_string())
    };
    let down = event(NSEventType::KeyDown)?;
    let up = event(NSEventType::KeyUp)?;
    remember_sent(&down, &webview, edit_of(stroke));
    lend_keyboard(&window, &webview, || {
        // AppKit hands command chords to performKeyEquivalent, never to keyDown.
        if !stroke.flags.contains(NSEventModifierFlags::Command)
            || !webview.performKeyEquivalent(&down)
        {
            webview.keyDown(&down);
        }
        webview.keyUp(&up);
    });
    Ok(())
}

fn remember_sent(event: &NSEvent, tab: &WKWebView, edit: Option<Edit>) {
    SENT_KEYS.with(|sent| {
        let mut sent = sent.borrow_mut();
        sent.retain(|key| key.at.elapsed() < SENT_KEY_LIFETIME);
        sent.push(SentKey {
            event: event.retain(),
            tab: Weak::from(tab),
            edit,
            at: Instant::now(),
        });
    });
}

/// Called for every key-down the app is about to handle. An agent key the
/// page left unhandled is kept from the app, and an editing chord among them
/// is done in the tab, as the Edit menu would for the person.
pub fn stop_returned_key(event: &NSEvent) -> bool {
    let returned = SENT_KEYS.with(|sent| {
        let mut sent = sent.borrow_mut();
        let index = sent
            .iter()
            .position(|key| std::ptr::eq(&*key.event, event))?;
        Some(sent.remove(index))
    });
    let Some(returned) = returned else {
        return false;
    };
    if let (Some(edit), Some(tab)) = (returned.edit, returned.tab.load()) {
        perform(&tab, edit);
    }
    true
}

fn edit_of(stroke: &KeyStroke) -> Option<Edit> {
    let command = NSEventModifierFlags::Command;
    let shifted = command | NSEventModifierFlags::Shift;
    match (stroke.unmodified.as_str(), stroke.flags) {
        ("a", flags) if flags == command => Some(Edit::SelectAll),
        ("c", flags) if flags == command => Some(Edit::Copy),
        ("x", flags) if flags == command => Some(Edit::Cut),
        ("v", flags) if flags == command => Some(Edit::Paste),
        ("z", flags) if flags == command => Some(Edit::Undo),
        ("z", flags) if flags == shifted => Some(Edit::Redo),
        _ => None,
    }
}

fn perform(tab: &WKWebView, edit: Edit) {
    let action = match edit {
        Edit::SelectAll => sel!(selectAll:),
        Edit::Copy => sel!(copy:),
        Edit::Cut => sel!(cut:),
        Edit::Paste => sel!(paste:),
        Edit::Undo | Edit::Redo => {
            // WKWebView has no `undo:` of its own; the page's history is
            // reached through WebKit's editing command of the same name.
            let command = sel!(_executeEditCommand:argument:completion:);
            if tab.respondsToSelector(command) {
                let name = NSString::from_str(if edit == Edit::Undo { "Undo" } else { "Redo" });
                // SAFETY: `respondsToSelector` just confirmed this method. It takes a
                // command name, an optional argument and an optional completion block,
                // and returns nothing. Main thread, and `tab` is retained.
                let _: () = unsafe {
                    msg_send![tab, _executeEditCommand: &*name, argument: std::ptr::null::<NSString>(), completion: std::ptr::null::<AnyObject>()]
                };
            }
            return;
        }
    };
    // SAFETY: `tryToPerform:with:` only sends the action if the tab answers to it,
    // and every editing action takes one sender and returns nothing.
    let _ = unsafe { tab.tryToPerform_with(action, None) };
}

fn inside(responder: &NSResponder, tab: &WKWebView) -> bool {
    responder
        .downcast_ref::<NSView>()
        .is_some_and(|view| view.isDescendantOf(tab))
}

/// A key only types from the window's first responder, so the tab holds it
/// for one key, too briefly for any of the person's keys to land in between.
/// Neither page hears of it: a page given the keyboard back moves the caret
/// to the start of the field.
fn lend_keyboard(window: &NSWindow, tab: &WKWebView, press: impl FnOnce()) {
    let person = window.firstResponder();
    if person
        .as_deref()
        .is_some_and(|responder| inside(responder, tab))
    {
        press();
        return;
    }
    let person = person.map(owner_of);
    LENDING_KEYBOARD.set(true);
    window.makeFirstResponder(Some(tab));
    LENDING_KEYBOARD.set(false);
    press();
    LENDING_KEYBOARD.set(true);
    window.makeFirstResponder(person.as_deref());
    LENDING_KEYBOARD.set(false);
}

/// A text field edits through a shared field editor, which is the first
/// responder only while that field is; the field itself is what to go back to.
fn owner_of(responder: Retained<NSResponder>) -> Retained<NSResponder> {
    if !responder.respondsToSelector(sel!(isFieldEditor)) {
        return responder;
    }
    // SAFETY: `respondsToSelector` just confirmed `isFieldEditor`, which takes nothing
    // and returns a BOOL. Main thread, and `responder` is retained.
    let field_editor: bool = unsafe { msg_send![&*responder, isFieldEditor] };
    if !field_editor || !responder.respondsToSelector(sel!(delegate)) {
        return responder;
    }
    // SAFETY: a field editor's `delegate` takes nothing and returns the field it
    // edits for, or nil. The result is retained.
    let delegate: Option<Retained<AnyObject>> = unsafe { msg_send![&*responder, delegate] };
    delegate
        .and_then(|delegate| delegate.downcast::<NSResponder>().ok())
        .unwrap_or(responder)
}

/// Whether `responder` taking the window's keyboard would move it into a tab
/// the person did not click into. A page focusing a field, on load or when
/// the agent clicks it, has WebKit ask for the keyboard, and would take it
/// from wherever the person is typing.
fn takes_keyboard_into_tab(window: &NSWindow, responder: &NSResponder) -> bool {
    let Some(tab) = responder
        .downcast_ref::<NSView>()
        .and_then(crate::macos::tab_holding)
    else {
        return false;
    };
    let tab_has_it = window
        .firstResponder()
        .is_some_and(|current| inside(&current, &tab));
    let person_clicked_it = PERSON_CLICKED.with(|clicked| {
        clicked
            .borrow()
            .as_ref()
            .and_then(Weak::load)
            .is_some_and(|view| view.isDescendantOf(&tab))
    });
    !tab_has_it && !person_clicked_it
}

type MakeFirstResponder = unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) -> Bool;
type FirstResponderChange = unsafe extern "C-unwind" fn(*mut AnyObject, Sel) -> Bool;

// SAFETY: only the Objective-C runtime calls this, as NSWindow's `makeFirstResponder:`,
// with a live window and a responder or nil.
unsafe extern "C-unwind" fn make_first_responder(
    window: *mut AnyObject,
    selector: Sel,
    responder: *mut AnyObject,
) -> Bool {
    // SAFETY: the runtime passes a live NSWindow, and the responder is nil or live.
    let (window_ref, responder_ref) = unsafe {
        (
            &*window.cast::<NSWindow>(),
            responder.cast::<NSResponder>().as_ref(),
        )
    };
    if !LENDING_KEYBOARD.get() {
        if let Some(responder) = responder_ref {
            if takes_keyboard_into_tab(window_ref, responder) {
                return Bool::NO;
            }
            if responder
                .downcast_ref::<NSView>()
                .and_then(crate::macos::tab_holding)
                .is_none()
            {
                PERSON_CLICKED.with(|clicked| clicked.borrow_mut().take());
            }
        }
    }
    let Some(original) = ORIGINAL_MAKE_FIRST_RESPONDER.get() else {
        return Bool::NO;
    };
    // SAFETY: `original` is the implementation this function replaced, so it takes
    // the same arguments.
    unsafe {
        std::mem::transmute::<Imp, MakeFirstResponder>(*original)(window, selector, responder)
    }
}

// SAFETY: only the Objective-C runtime calls this, as WKWebView's `becomeFirstResponder`.
unsafe extern "C-unwind" fn become_first_responder(view: *mut AnyObject, selector: Sel) -> Bool {
    if LENDING_KEYBOARD.get() {
        return Bool::YES;
    }
    let Some(original) = ORIGINAL_BECOME_FIRST_RESPONDER.get() else {
        return Bool::YES;
    };
    // SAFETY: `original` is the implementation this function replaced, so it takes
    // the same arguments.
    unsafe { std::mem::transmute::<Imp, FirstResponderChange>(*original)(view, selector) }
}

// SAFETY: only the Objective-C runtime calls this, as WKWebView's `resignFirstResponder`.
unsafe extern "C-unwind" fn resign_first_responder(view: *mut AnyObject, selector: Sel) -> Bool {
    if LENDING_KEYBOARD.get() {
        return Bool::YES;
    }
    let Some(original) = ORIGINAL_RESIGN_FIRST_RESPONDER.get() else {
        return Bool::YES;
    };
    // SAFETY: `original` is the implementation this function replaced, so it takes
    // the same arguments.
    unsafe { std::mem::transmute::<Imp, FirstResponderChange>(*original)(view, selector) }
}

/// Keeps the person's keyboard where they put it while agents drive tabs:
/// a tab takes it only from the person's own click, and a key the agent
/// lends a tab is never noticed by either page. Call once, on the main thread.
pub fn guard_keyboard() {
    if let Some(method) = AnyClass::get(c"NSWindow")
        .and_then(|class| class.instance_method(sel!(makeFirstResponder:)))
    {
        let replacement: MakeFirstResponder = make_first_responder;
        // SAFETY: `make_first_responder` has the signature of `-[NSWindow
        // makeFirstResponder:]` and calls the implementation it replaces, which is
        // stored before anything can call it.
        let original = unsafe {
            method.set_implementation(std::mem::transmute::<MakeFirstResponder, Imp>(replacement))
        };
        let _ = ORIGINAL_MAKE_FIRST_RESPONDER.set(original);
    }
    let Some(webview) = AnyClass::get(c"WKWebView") else {
        return;
    };
    for (selector, replacement, slot) in [
        (
            sel!(becomeFirstResponder),
            become_first_responder as FirstResponderChange,
            &ORIGINAL_BECOME_FIRST_RESPONDER,
        ),
        (
            sel!(resignFirstResponder),
            resign_first_responder as FirstResponderChange,
            &ORIGINAL_RESIGN_FIRST_RESPONDER,
        ),
    ] {
        if let Some(method) = webview.instance_method(selector) {
            // SAFETY: both replacements have the signature of the NSResponder method
            // they replace and call the implementation stored here.
            let original = unsafe {
                method.set_implementation(std::mem::transmute::<FirstResponderChange, Imp>(
                    replacement,
                ))
            };
            let _ = slot.set(original);
        }
    }
}

/// When the agent and the person last used the pointer and the keyboard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct LastInput {
    agent_pointer: Option<Instant>,
    agent_key: Option<Instant>,
    person_pointer: Option<Instant>,
    person_key: Option<Instant>,
}

impl LastInput {
    const NONE: Self = Self {
        agent_pointer: None,
        agent_key: None,
        person_pointer: None,
        person_key: None,
    };

    /// WebKit answers a pointer event with the cursor for that point and a key
    /// with hiding the cursor, and does so for the whole app. The answer to the
    /// agent's input is dropped until the person's own input takes over again.
    fn cursor_follows_agent(agent: Option<Instant>, person: Option<Instant>, now: Instant) -> bool {
        agent.is_some_and(|agent| {
            now.duration_since(agent) < CURSOR_ANSWER_WINDOW
                && person.is_none_or(|person| person < agent)
        })
    }

    fn keeps_cursor_shape(&self, now: Instant) -> bool {
        Self::cursor_follows_agent(self.agent_pointer, self.person_pointer, now)
    }

    fn keeps_cursor_shown(&self, now: Instant) -> bool {
        Self::cursor_follows_agent(self.agent_key, self.person_key, now)
    }
}

fn note(update: impl FnOnce(&mut LastInput)) {
    LAST_INPUT.with(|cell| {
        let mut last = cell.get();
        update(&mut last);
        cell.set(last);
    });
}

/// Called for every pointer and key event the person makes in the app.
pub fn note_person(event: &NSEvent) {
    let now = Instant::now();
    match event.r#type() {
        NSEventType::KeyDown | NSEventType::FlagsChanged => {
            note(|last| last.person_key = Some(now))
        }
        NSEventType::LeftMouseDown | NSEventType::RightMouseDown | NSEventType::OtherMouseDown => {
            note(|last| last.person_pointer = Some(now));
            let clicked = clicked_view(event);
            PERSON_CLICKED.with(|slot| *slot.borrow_mut() = clicked.as_deref().map(Weak::from));
        }
        _ => note(|last| last.person_pointer = Some(now)),
    }
}

fn clicked_view(event: &NSEvent) -> Option<Retained<NSView>> {
    let window = event.window(MainThreadMarker::new()?)?;
    window.contentView()?.hitTest(event.locationInWindow())
}

/// Whether the app's latest pointer or key input came from an agent rather
/// than the person.
pub fn agent_input_is_latest() -> bool {
    let last = LAST_INPUT.with(Cell::get);
    let now = Instant::now();
    last.keeps_cursor_shape(now) || last.keeps_cursor_shown(now)
}

type SetCursor = unsafe extern "C-unwind" fn(*mut AnyObject, Sel);
type HideCursor = unsafe extern "C-unwind" fn(*const AnyClass, Sel, Bool);

// SAFETY: only the Objective-C runtime calls this, as NSCursor's `set`, with a live
// cursor.
unsafe extern "C-unwind" fn set_cursor(cursor: *mut AnyObject, selector: Sel) {
    if LAST_INPUT
        .with(Cell::get)
        .keeps_cursor_shape(Instant::now())
    {
        return;
    }
    if let Some(original) = ORIGINAL_SET_CURSOR.get() {
        // SAFETY: `original` is the `set` implementation this function replaced,
        // so it takes the same arguments.
        unsafe { std::mem::transmute::<Imp, SetCursor>(*original)(cursor, selector) };
    }
}

// SAFETY: only the Objective-C runtime calls this, as NSCursor's class method
// `setHiddenUntilMouseMoves:`.
unsafe extern "C-unwind" fn hide_cursor(class: *const AnyClass, selector: Sel, hide: Bool) {
    if hide.as_bool()
        && LAST_INPUT
            .with(Cell::get)
            .keeps_cursor_shown(Instant::now())
    {
        return;
    }
    if let Some(original) = ORIGINAL_HIDE_CURSOR.get() {
        // SAFETY: `original` is the implementation this function replaced, so it
        // takes the same arguments.
        unsafe { std::mem::transmute::<Imp, HideCursor>(*original)(class, selector, hide) };
    }
}

/// Keeps the person's cursor from changing shape or hiding in answer to the
/// agent's input: WebKit sets the cursor for the whole app whenever the real
/// pointer is anywhere over the window. Call once, on the main thread.
pub fn guard_cursor() {
    let Some(class) = AnyClass::get(c"NSCursor") else {
        return;
    };
    if let Some(method) = class.instance_method(sel!(set)) {
        let replacement: SetCursor = set_cursor;
        // SAFETY: `set_cursor` has the signature of `-[NSCursor set]` and calls the
        // implementation it replaces, which is stored before anything can call it.
        let original = unsafe {
            method.set_implementation(std::mem::transmute::<SetCursor, Imp>(replacement))
        };
        let _ = ORIGINAL_SET_CURSOR.set(original);
    }
    if let Some(method) = class.class_method(sel!(setHiddenUntilMouseMoves:)) {
        let replacement: HideCursor = hide_cursor;
        // SAFETY: `hide_cursor` has the signature of `+[NSCursor
        // setHiddenUntilMouseMoves:]` and calls the implementation it replaces.
        let original = unsafe {
            method.set_implementation(std::mem::transmute::<HideCursor, Imp>(replacement))
        };
        let _ = ORIGINAL_HIDE_CURSOR.set(original);
    }
}

/// WebKit only notices a hung page when input it sent goes unanswered for a
/// few seconds, and a script call is not input. This sends a key release no
/// key matches, which pages ignore, so `BrowserManager::stalled` can tell.
pub fn probe_responsiveness(webview: &WKWebView) -> Result<(), String> {
    let webview = webview.retain();
    let window = webview.window().ok_or("the tab is not in a window")?;
    let release = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
        NSEventType::KeyUp,
        NSPoint::new(0.0, 0.0),
        NSEventModifierFlags::empty(),
        now(),
        window.windowNumber(),
        None,
        &NSString::new(),
        &NSString::new(),
        false,
        UNUSED_KEY_CODE,
    )
    .ok_or("AppKit refused the key event")?;
    webview.keyUp(&release);
    Ok(())
}

/// Inserts text at the page's caret the way a keyboard or input method would,
/// so editors that keep their own model of the document see it arrive. Unlike
/// a key, this reaches the page without the window's keyboard.
pub fn insert_text(webview: &WKWebView, text: &str) -> Result<(), String> {
    let webview = webview.retain();
    note(|last| last.agent_key = Some(Instant::now()));
    let text = NSString::from_str(text);
    // SAFETY: WKWebView implements NSTextInputClient's `insertText:`, which takes one
    // string and returns nothing. Main thread, and both objects are retained.
    let _: () = unsafe { msg_send![&*webview, insertText: &*text] };
    Ok(())
}

pub fn parse_key(name: &str) -> Result<KeyStroke, String> {
    use crate::keys::{Key, Named};

    let chord = crate::keys::parse(name)?;
    let held = chord.modifiers;
    let mut flags = NSEventModifierFlags::empty();
    for (on, flag) in [
        (held.meta, NSEventModifierFlags::Command),
        (held.control, NSEventModifierFlags::Control),
        (held.alt, NSEventModifierFlags::Option),
        (held.shift, NSEventModifierFlags::Shift),
    ] {
        if on {
            flags |= flag;
        }
    }
    let named = |code: u16, character: char| (code, character.to_string(), character.to_string());
    let (code, characters, unmodified) = match chord.key {
        Key::Named(Named::Enter) => named(36, '\r'),
        Key::Named(Named::Tab) => named(48, '\t'),
        Key::Named(Named::Escape) => named(53, '\u{1b}'),
        Key::Named(Named::Backspace) => named(51, '\u{7f}'),
        Key::Named(Named::Delete) => named(117, '\u{F728}'),
        Key::Named(Named::Space) => named(49, ' '),
        Key::Named(Named::ArrowUp) => named(126, '\u{F700}'),
        Key::Named(Named::ArrowDown) => named(125, '\u{F701}'),
        Key::Named(Named::ArrowLeft) => named(123, '\u{F702}'),
        Key::Named(Named::ArrowRight) => named(124, '\u{F703}'),
        Key::Named(Named::Home) => named(115, '\u{F729}'),
        Key::Named(Named::End) => named(119, '\u{F72B}'),
        Key::Named(Named::PageUp) => named(116, '\u{F72C}'),
        Key::Named(Named::PageDown) => named(121, '\u{F72D}'),
        Key::Char(character) => {
            let lower = character.to_lowercase().collect::<String>();
            let shown = if held.shift {
                character.to_uppercase().collect()
            } else {
                character.to_string()
            };
            (key_code(&lower), shown, lower)
        }
    };
    Ok(KeyStroke {
        code,
        characters,
        unmodified,
        flags,
    })
}

/// Virtual key codes on a US layout. Pages read them through `event.code`.
fn key_code(character: &str) -> u16 {
    const LAYOUT: &[(&str, u16)] = &[
        ("a", 0),
        ("s", 1),
        ("d", 2),
        ("f", 3),
        ("h", 4),
        ("g", 5),
        ("z", 6),
        ("x", 7),
        ("c", 8),
        ("v", 9),
        ("b", 11),
        ("q", 12),
        ("w", 13),
        ("e", 14),
        ("r", 15),
        ("y", 16),
        ("t", 17),
        ("1", 18),
        ("2", 19),
        ("3", 20),
        ("4", 21),
        ("6", 22),
        ("5", 23),
        ("=", 24),
        ("9", 25),
        ("7", 26),
        ("-", 27),
        ("8", 28),
        ("0", 29),
        ("]", 30),
        ("o", 31),
        ("u", 32),
        ("[", 33),
        ("i", 34),
        ("p", 35),
        ("l", 37),
        ("j", 38),
        ("'", 39),
        ("k", 40),
        (";", 41),
        ("\\", 42),
        (",", 43),
        ("/", 44),
        ("n", 45),
        ("m", 46),
        (".", 47),
        ("`", 50),
    ];
    LAYOUT
        .iter()
        .find(|(key, _)| *key == character)
        .map(|(_, code)| *code)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_keys_carry_the_function_key_characters_webkit_expects() {
        let enter = parse_key("Enter").unwrap();
        assert_eq!((enter.code, enter.characters.as_str()), (36, "\r"));
        assert_eq!(parse_key("ArrowDown").unwrap().characters, "\u{F701}");
        assert!(parse_key("Hyper").is_err());
    }

    #[test]
    fn chords_split_into_modifiers_and_one_key() {
        let select_all = parse_key("Meta+a").unwrap();
        assert_eq!(select_all.flags, NSEventModifierFlags::Command);
        assert_eq!((select_all.code, select_all.unmodified.as_str()), (0, "a"));
        let back_tab = parse_key("Shift+Tab").unwrap();
        assert_eq!(back_tab.flags, NSEventModifierFlags::Shift);
        assert_eq!(back_tab.code, 48);
        assert_eq!(parse_key("Shift+k").unwrap().characters, "K");
        assert_eq!(parse_key("Control++").unwrap().characters, "+");
        assert!(parse_key("Super+a").is_err());
    }

    #[test]
    fn the_cursor_ignores_the_agent_until_the_person_moves_or_types() {
        let start = Instant::now();
        let later = |ms| start + Duration::from_millis(ms);
        let mut last = LastInput::NONE;
        assert!(!last.keeps_cursor_shape(start));

        last.agent_pointer = Some(start);
        assert!(last.keeps_cursor_shape(later(100)));
        assert!(!last.keeps_cursor_shown(later(100)));
        assert!(!last.keeps_cursor_shape(start + CURSOR_ANSWER_WINDOW));

        last.person_pointer = Some(later(50));
        assert!(!last.keeps_cursor_shape(later(100)));

        last.agent_key = Some(later(200));
        last.person_key = Some(later(10));
        assert!(last.keeps_cursor_shown(later(300)));
        last.person_key = Some(later(250));
        assert!(!last.keeps_cursor_shown(later(300)));
    }

    #[test]
    fn only_the_plain_editing_chords_become_editing_commands() {
        let edit = |name: &str| edit_of(&parse_key(name).unwrap());
        assert_eq!(edit("Meta+a"), Some(Edit::SelectAll));
        assert_eq!(edit("Meta+c"), Some(Edit::Copy));
        assert_eq!(edit("Meta+x"), Some(Edit::Cut));
        assert_eq!(edit("Meta+v"), Some(Edit::Paste));
        assert_eq!(edit("Meta+z"), Some(Edit::Undo));
        assert_eq!(edit("Shift+Meta+z"), Some(Edit::Redo));
        assert_eq!(edit("Meta+t"), None);
        assert_eq!(edit("Control+a"), None);
        assert_eq!(edit("Alt+Meta+a"), None);
        assert_eq!(edit("a"), None);
    }
}
