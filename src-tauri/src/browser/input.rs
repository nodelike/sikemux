//! Mouse and keyboard input for a tab, delivered as real AppKit events. A page
//! can tell a script-dispatched event from a person's (`isTrusted`), and rich
//! editors ignore values written straight into the DOM, so the agent's clicks
//! and keys take the same path through WebKit as the person's.
//!
//! Events go straight to the tab's view rather than through the window, so a
//! tab that is off screen still receives them. Keys only reach the page from
//! the window's first responder, and a click makes the tab the first responder
//! on its own, so the person's keyboard focus is held at the start of an agent
//! action and given back at the end.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::{msg_send, sel, Message};
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType, NSResponder, NSView, NSWindow};
use objc2_foundation::{NSObjectProtocol, NSPoint, NSProcessInfo, NSString};
use objc2_web_kit::WKWebView;

/// WebKit hands a key back to the app once the page is done with it, which
/// can take a while on a busy page.
const SENT_KEY_LIFETIME: Duration = Duration::from_secs(30);
/// WebKit takes the keyboard for a clicked page a moment after the click.
const LATE_FOCUS_GRAB: Duration = Duration::from_millis(400);
/// How long after an agent's input WebKit may still answer it with a cursor.
const CURSOR_ANSWER_WINDOW: Duration = Duration::from_secs(3);
/// No keyboard has a key with this code.
const UNUSED_KEY_CODE: u16 = 0xFF;

static ORIGINAL_SET_CURSOR: OnceLock<Imp> = OnceLock::new();
static ORIGINAL_HIDE_CURSOR: OnceLock<Imp> = OnceLock::new();

thread_local! {
    static SENT_KEYS: RefCell<Vec<SentKey>> = const { RefCell::new(Vec::new()) };
    static HELD_FOCUS: RefCell<HashMap<isize, HeldFocus>> = RefCell::new(HashMap::new());
    static LAST_INPUT: Cell<LastInput> = const { Cell::new(LastInput::NONE) };
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

/// Where the person's keyboard focus was when an agent action began.
struct HeldFocus {
    responder: Retained<NSResponder>,
    holders: usize,
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

fn webview_from(pointer: *mut c_void) -> Result<Retained<WKWebView>, String> {
    // SAFETY: every caller passes `platform.inner()` from inside Tauri's `with_webview`,
    // which runs on the main thread while that WKWebView is alive.
    unsafe { Retained::retain(pointer.cast::<WKWebView>()) }.ok_or_else(|| "the tab is gone".into())
}

fn now() -> f64 {
    NSProcessInfo::processInfo().systemUptime()
}

/// `x` and `y` are CSS pixels from the top left of the page's viewport. The
/// view's bounds are in page pixels even while it is drawn scaled, so only
/// the page's own zoom stands between the two.
pub fn mouse(
    pointer: *mut c_void,
    kind: Mouse,
    x: f64,
    y: f64,
    clicks: isize,
) -> Result<(), String> {
    let webview = webview_from(pointer)?;
    let window = webview.window().ok_or("the tab is not in a window")?;
    note(|last| last.agent_pointer = Some(Instant::now()));
    // SAFETY: main thread (see `webview_from`), and `webview` is retained.
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

pub fn key(pointer: *mut c_void, stroke: &KeyStroke) -> Result<(), String> {
    let webview = webview_from(pointer)?;
    let window = webview.window().ok_or("the tab is not in a window")?;
    take_keyboard(&window, &webview);
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
    // AppKit hands command chords to performKeyEquivalent, never to keyDown.
    if !stroke.flags.contains(NSEventModifierFlags::Command) || !webview.performKeyEquivalent(&down)
    {
        webview.keyDown(&down);
    }
    webview.keyUp(&up);
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

/// Keys resolve against the window's first responder, not the view they
/// are sent to, so the tab has to hold the keyboard while one is pressed.
fn take_keyboard(window: &NSWindow, tab: &WKWebView) {
    let holds = window
        .firstResponder()
        .is_some_and(|responder| inside(&responder, tab));
    if !holds {
        window.makeFirstResponder(Some(tab));
    }
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

/// Notes where the person's keyboard focus is as an agent action begins, so
/// `return_person_focus` can give it back. Nested and overlapping actions in
/// one window share the first note.
pub fn hold_person_focus(pointer: *mut c_void) -> Result<(), String> {
    let tab = webview_from(pointer)?;
    let window = tab.window().ok_or("the tab is not in a window")?;
    let number = window.windowNumber();
    HELD_FOCUS.with(|held| {
        let mut held = held.borrow_mut();
        if let Some(focus) = held.get_mut(&number) {
            focus.holders += 1;
            return;
        }
        let Some(responder) = window.firstResponder() else {
            return;
        };
        if inside(&responder, &tab) {
            return;
        }
        held.insert(
            number,
            HeldFocus {
                responder: owner_of(responder),
                holders: 1,
            },
        );
    });
    Ok(())
}

/// Gives the keyboard back to where `hold_person_focus` found it, but only
/// if this tab still has it: the person may have moved on meanwhile. WebKit
/// takes the keyboard for a clicked page a moment late, so this looks again
/// shortly after.
pub fn return_person_focus(pointer: *mut c_void) -> Result<(), String> {
    let tab = webview_from(pointer)?;
    let window = tab.window().ok_or("the tab is not in a window")?;
    let number = window.windowNumber();
    let person = HELD_FOCUS.with(|held| {
        let mut held = held.borrow_mut();
        let focus = held.get_mut(&number)?;
        focus.holders -= 1;
        if focus.holders > 0 {
            return None;
        }
        held.remove(&number).map(|focus| focus.responder)
    });
    let Some(person) = person else {
        return Ok(());
    };
    give_back(&window, &tab, &person);
    after(
        LATE_FOCUS_GRAB,
        Box::new(move || {
            let acting_again = HELD_FOCUS.with(|held| held.borrow().contains_key(&number));
            if !acting_again {
                give_back(&window, &tab, &person);
            }
        }),
    );
    Ok(())
}

fn give_back(window: &NSWindow, tab: &WKWebView, person: &NSResponder) {
    let agent_has_it = window
        .firstResponder()
        .is_some_and(|responder| inside(&responder, tab));
    if agent_has_it {
        window.makeFirstResponder(Some(person));
    }
}

type MainThreadWork = Box<dyn FnOnce()>;

extern "C" {
    static _dispatch_main_q: c_void;
    fn dispatch_time(when: u64, delta: i64) -> u64;
    fn dispatch_after_f(
        when: u64,
        queue: *const c_void,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
}

/// Runs `work` on the main thread once `delay` has passed.
fn after(delay: Duration, work: MainThreadWork) {
    extern "C" fn run(context: *mut c_void) {
        // SAFETY: `context` is the box `after` leaked below, and the main queue
        // runs this exactly once.
        let work = unsafe { Box::from_raw(context.cast::<MainThreadWork>()) };
        work();
    }
    let context = Box::into_raw(Box::new(work)).cast::<c_void>();
    let delta = i64::try_from(delay.as_nanos()).unwrap_or(i64::MAX);
    // SAFETY: `_dispatch_main_q` is libdispatch's main queue, alive for the whole
    // process. `run` takes back ownership of `context`, and the main queue only runs
    // it on the main thread, where the objects the work holds belong.
    unsafe {
        dispatch_after_f(
            dispatch_time(0, delta),
            std::ptr::addr_of!(_dispatch_main_q),
            context,
            run,
        );
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
        _ => note(|last| last.person_pointer = Some(now)),
    }
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
pub fn probe_responsiveness(pointer: *mut c_void) -> Result<(), String> {
    let webview = webview_from(pointer)?;
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
/// so editors that keep their own model of the document see it arrive.
pub fn insert_text(pointer: *mut c_void, text: &str) -> Result<(), String> {
    let webview = webview_from(pointer)?;
    if let Some(window) = webview.window() {
        take_keyboard(&window, &webview);
    }
    note(|last| last.agent_key = Some(Instant::now()));
    let text = NSString::from_str(text);
    // SAFETY: WKWebView implements NSTextInputClient's `insertText:`, which takes one
    // string and returns nothing. Main thread, and both objects are retained.
    let _: () = unsafe { msg_send![&*webview, insertText: &*text] };
    Ok(())
}

pub fn parse_key(name: &str) -> Result<KeyStroke, String> {
    let mut flags = NSEventModifierFlags::empty();
    let mut rest = name;
    while let Some((modifier, tail)) = rest.split_once('+').filter(|(_, tail)| !tail.is_empty()) {
        flags |= match modifier.to_ascii_lowercase().as_str() {
            "meta" | "cmd" | "command" => NSEventModifierFlags::Command,
            "control" | "ctrl" => NSEventModifierFlags::Control,
            "alt" | "option" => NSEventModifierFlags::Option,
            "shift" => NSEventModifierFlags::Shift,
            _ => return Err(format!("unknown modifier \"{modifier}\" in \"{name}\"")),
        };
        rest = tail;
    }
    let named =
        |code: u16, character: char| Some((code, character.to_string(), character.to_string()));
    let (code, characters, unmodified) = match rest {
        "Enter" | "Return" => named(36, '\r'),
        "Tab" => named(48, '\t'),
        "Escape" | "Esc" => named(53, '\u{1b}'),
        "Backspace" => named(51, '\u{7f}'),
        "Delete" => named(117, '\u{F728}'),
        "Space" | " " => named(49, ' '),
        "ArrowUp" => named(126, '\u{F700}'),
        "ArrowDown" => named(125, '\u{F701}'),
        "ArrowLeft" => named(123, '\u{F702}'),
        "ArrowRight" => named(124, '\u{F703}'),
        "Home" => named(115, '\u{F729}'),
        "End" => named(119, '\u{F72B}'),
        "PageUp" => named(116, '\u{F72C}'),
        "PageDown" => named(121, '\u{F72D}'),
        _ => {
            let mut chars = rest.chars();
            match (chars.next(), chars.next()) {
                (Some(character), None) => {
                    let lower = character.to_lowercase().collect::<String>();
                    let shown = if flags.contains(NSEventModifierFlags::Shift) {
                        character.to_uppercase().collect()
                    } else {
                        character.to_string()
                    };
                    Some((key_code(&lower), shown, lower))
                }
                _ => None,
            }
        }
    }
    .ok_or_else(|| format!("unknown key \"{rest}\"; use a single character or a name like Enter, Tab, Escape, ArrowDown"))?;
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
