//! The app's shortcuts. Each of today's command ids is bound to the GPUI action of that name,
//! with the keys the person chose in the core's preferences, else the defaults both apps read.
//! An id no part of the app has an action for yet stays unbound.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::LazyLock;

use gpui::KeyBindingContextPredicate;
use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
use gpui_kit::{
    actions, App, Global, KeyBinding, Keystroke, Menu, MenuItem, OsAction, SharedString,
    SystemMenuType, Window,
};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;

use crate::core::Ask;

static TABLE: &str = include_str!("../../crates/sikemux-core/src/assets/keybindings.json");

/// The window's own context. The app's shortcuts work under it, so a view in focus with its
/// own binding for the same keys, such as the editor's ⌘[, takes them first.
pub const SHELL: &str = "Workbench";

/// The shortcuts that still work while a palette, picker or other view with `Modal` in its
/// context has the keys, as on the Mac: the ones that open another palette or Settings.
const OVER_MODALS: [&str; 4] = [
    "palette.commands",
    "palette.files",
    "search.global",
    "settings.toggle",
];

/// Shortcuts that leave Settings open; any other closes it first.
const KEEP_SETTINGS: [&str; 3] = ["text.sizeIncrease", "text.sizeDecrease", "text.sizeReset"];

/// What closing means while Settings covers the window: closing Settings, not a tab or
/// project hidden behind it.
const CLOSE_SETTINGS: [&str; 2] = ["pane.close", "session.close"];

/// Commands that act on a desk's page only while the keys are in it, leaving the same keys to
/// the editor and everything else.
const PAGE_ONLY: [&str; 3] = ["browser.reload", "browser.back", "browser.forward"];

/// Where the shortcut of command `id` works: the welcome takes even the palettes' keys. Keys
/// held with Option alone stay with a terminal or text field being typed in, since shells read
/// Option as Meta and other keyboard layouts type with it.
fn context(id: &str, keys: &str) -> String {
    let place = if PAGE_ONLY.contains(&id) {
        "BrowserPage"
    } else if OVER_MODALS.contains(&id) {
        "Workbench && !Onboarding"
    } else {
        "Workbench && !Modal"
    };
    let held: Vec<&str> = keys.split('+').collect();
    let option_only = held.contains(&"Alt") && !held.contains(&"Meta") && !held.contains(&"Ctrl");
    match option_only {
        true => format!("{place} && !Terminal && !Input"),
        false => place.to_owned(),
    }
}

actions!(
    app,
    [
        About,
        Hide,
        HideOthers,
        ShowAll,
        Quit,
        Minimize,
        Zoom,
        ToggleFullScreen
    ]
);

/// Defines unit actions named by today's command ids.
macro_rules! commands {
    ($($(#[$doc:meta])* $name:ident = $id:literal,)*) => {$(
        $(#[$doc])*
        #[derive(Clone, Default, PartialEq, gpui::Action)]
        #[action(name = $id)]
        pub struct $name;
    )*};
}

commands! {
    PreviousTab = "window.previous",
    NextTab = "window.next",
    /// The previous tab showing the same kind of pane as the one in front.
    PreviousOfKind = "tab.previous",
    NextOfKind = "tab.next",
    GoToTab1 = "tab.goto1",
    GoToTab2 = "tab.goto2",
    GoToTab3 = "tab.goto3",
    GoToTab4 = "tab.goto4",
    GoToTab5 = "tab.goto5",
    GoToTab6 = "tab.goto6",
    GoToTab7 = "tab.goto7",
    GoToTab8 = "tab.goto8",
    /// The last tab, wherever it is.
    GoToLastTab = "tab.goto9",
    CloseTab = "pane.close",
    /// Hides both rails.
    FocusMode = "view.focusMode",
    /// Starts a chat with the agent the rail is set to.
    NewChat = "agent.new",
    /// Brings the project's terminal forward, opening one when it has none.
    FocusTerminal = "window.terminal",
    /// Bigger text in the chat, editor or terminal that has the focus.
    TextSizeIncrease = "text.sizeIncrease",
    TextSizeDecrease = "text.sizeDecrease",
    TextSizeReset = "text.sizeReset",
    /// Lets the agent in front run without asking, or has it ask again.
    ToggleYolo = "agent.permissions",
    /// A new shell beside the view in focus, to its right or below it, or behind it as a tab.
    SplitRow = "pane.splitRow",
    SplitColumn = "pane.splitColumn",
    SplitStack = "pane.splitStack",
    FocusLeft = "pane.focusLeft",
    FocusRight = "pane.focusRight",
    FocusUp = "pane.focusUp",
    FocusDown = "pane.focusDown",
    ResizeLeft = "pane.resizeLeft",
    ResizeRight = "pane.resizeRight",
    ResizeUp = "pane.resizeUp",
    ResizeDown = "pane.resizeDown",
    /// The view in focus fills its tab, or gives the room back.
    ZoomPane = "pane.zoom",
}

actions!(
    rail_resizer,
    [EdgeLeft, EdgeRight, EdgeFarLeft, EdgeFarRight]
);
actions!(retitle, [CancelRetitle]);

/// Stops every agent and terminal the core runs, then quits.
#[derive(Clone, Default, PartialEq, gpui::Action)]
#[action(name = "app.quitAndStopEverything")]
pub struct QuitAndStopEverything;

/// One command, as the shared table names, describes and binds it by default.
#[derive(Deserialize)]
pub struct Shortcut {
    pub id: &'static str,
    pub label: &'static str,
    pub detail: &'static str,
    pub category: &'static str,
    mac: Option<&'static str>,
    other: Option<&'static str>,
}

impl Shortcut {
    pub fn default_keys(&self) -> Option<&'static str> {
        match cfg!(target_os = "macos") {
            true => self.mac,
            false => self.other,
        }
    }

    /// The keys the person chose for it, else its default.
    pub fn keys(&self, overrides: &BTreeMap<String, Option<String>>) -> Option<String> {
        match overrides.get(self.id) {
            Some(chosen) => chosen.clone(),
            None => self.default_keys().map(str::to_owned),
        }
    }
}

/// The categories shortcuts are listed under, in the order they are shown.
pub const CATEGORIES: &[&str] = &["Workspace", "Agents", "Panes", "Navigation", "Browser"];

/// Every command in the shared table.
pub fn shortcuts() -> &'static [Shortcut] {
    static SHORTCUTS: LazyLock<Vec<Shortcut>> =
        LazyLock::new(|| serde_json::from_str(TABLE).expect("the shortcut table reads"));
    &SHORTCUTS
}

/// The keys each command is bound to now in the table's own spelling, such as `Meta+Shift+KeyP`,
/// and the keys the project in front's own actions are bound to, by action id.
pub struct Keymap {
    bound: BTreeMap<String, String>,
    actions: BTreeMap<String, String>,
    overrides: Option<BTreeMap<String, Option<String>>>,
    /// Where each command the person runs is told, for the usage report.
    used: Option<mpsc::UnboundedSender<Ask>>,
}

/// One of the project in front's sikemux.json actions, run by the shortcut it gives itself.
#[derive(Clone, PartialEq, gpui::Action)]
#[action(name = "project.runAction", no_json)]
pub struct RunProjectAction {
    pub id: String,
    pub label: String,
}

/// Binds the project in front's actions to their own shortcuts, as its sikemux.json gives them
/// in the table's spelling: `(id, label, keys)`. Keys one of the app's commands uses stay the
/// command's, as on the Mac, and keys an action no longer uses stop doing anything.
pub fn bind_project_actions(actions: &[(String, String, String)], cx: &mut App) {
    let keymap = cx.global::<Keymap>();
    let taken = |keys: &str| keymap.bound.values().any(|bound| bound == keys);
    let mut bound = BTreeMap::new();
    let mut bindings = Vec::new();
    for (id, label, keys) in actions {
        if taken(keys) || bound.values().any(|kept: &String| kept == keys) {
            continue;
        }
        let Some(gpui_keys) = keystroke(keys) else {
            continue;
        };
        let action = RunProjectAction {
            id: id.clone(),
            label: label.clone(),
        };
        let context = KeyBindingContextPredicate::parse(&context(id, keys))
            .ok()
            .map(Rc::new);
        let Ok(binding) = KeyBinding::load(&gpui_keys, Box::new(action), context, false, None, cx)
        else {
            continue;
        };
        bindings.push(binding);
        bound.insert(id.clone(), keys.clone());
    }
    if bound == keymap.actions {
        return;
    }
    let run = <RunProjectAction as gpui::Action>::name_for_type();
    replace(bindings, |name| name == run, cx);
    cx.global_mut::<Keymap>().actions = bound;
}

/// Puts `bindings` in place of the ones whose action `ours` names, keeping every other view's,
/// since GPUI's keymap only grows and every key press walks all of it.
fn replace(bindings: Vec<KeyBinding>, ours: impl Fn(&str) -> bool, cx: &mut App) {
    let kept: Vec<KeyBinding> = cx
        .key_bindings()
        .borrow()
        .bindings()
        .filter(|binding| !ours(binding.action().name()))
        .cloned()
        .collect();
    cx.clear_key_bindings();
    cx.bind_keys(kept.into_iter().chain(bindings));
}

/// The keys the project in front's action `id` runs by, as the deck shows them.
pub fn action_hint(id: &str, cx: &App) -> Option<SharedString> {
    label(cx.try_global::<Keymap>()?.actions.get(id)?)
}

impl Global for Keymap {}

pub fn install(cx: &mut App) {
    cx.set_global(Keymap {
        bound: BTreeMap::new(),
        actions: BTreeMap::new(),
        overrides: None,
        used: None,
    });
    cx.on_action(|_: &About, _| show_about());
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    if cfg!(target_os = "macos") {
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-h", Hide, None),
            KeyBinding::new("alt-cmd-h", HideOthers, None),
            KeyBinding::new("alt-cmd-q", QuitAndStopEverything, None),
            KeyBinding::new("cmd-m", Minimize, None),
            KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None),
        ]);
    } else {
        cx.bind_keys([
            KeyBinding::new("ctrl-q", Quit, None),
            KeyBinding::new("ctrl-alt-q", QuitAndStopEverything, None),
            KeyBinding::new("f11", ToggleFullScreen, None),
        ]);
        // With no menu bar or dock to come back from, a closed window would leave the app
        // running unseen.
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
    }
    cx.bind_keys([
        KeyBinding::new("left", EdgeLeft, Some("RailResizer")),
        KeyBinding::new("right", EdgeRight, Some("RailResizer")),
        KeyBinding::new("shift-left", EdgeFarLeft, Some("RailResizer")),
        KeyBinding::new("shift-right", EdgeFarRight, Some("RailResizer")),
        KeyBinding::new("escape", CancelRetitle, Some("Retitle > Input")),
    ]);
    bind(&BTreeMap::new(), cx);
    cx.set_menus(menus());
}

/// Binds every command to the person's keys, else its default, again whenever they change.
pub fn bind(overrides: &BTreeMap<String, Option<String>>, cx: &mut App) {
    if cx.global::<Keymap>().overrides.as_ref() == Some(overrides) {
        return;
    }
    let mut bound = BTreeMap::new();
    let mut bindings = Vec::new();
    for shortcut in shortcuts() {
        let Some((keys, gpui_keys)) = shortcut
            .keys(overrides)
            .and_then(|keys| Some((keys.clone(), keystroke(&keys)?)))
        else {
            continue;
        };
        let Ok(action) = cx.build_action(shortcut.id, None) else {
            continue;
        };
        let context = KeyBindingContextPredicate::parse(&context(shortcut.id, &keys))
            .ok()
            .map(Rc::new);
        if let Ok(binding) = KeyBinding::load(&gpui_keys, action, context, false, None, cx) {
            bindings.push(binding);
            bound.insert(shortcut.id.to_owned(), keys);
        }
    }
    replace(
        bindings,
        |name| shortcuts().iter().any(|shortcut| shortcut.id == name),
        cx,
    );
    let keymap = cx.global_mut::<Keymap>();
    keymap.bound = bound;
    keymap.overrides = Some(overrides.clone());
}

/// Does what command `id` does, as its shortcut would; nothing while no part of the app has it.
pub fn dispatch(id: &str, window: &mut Window, cx: &mut App) {
    if let Ok(action) = cx.build_action(id, None) {
        used(id, cx);
        window.dispatch_action(action, cx);
    }
}

/// Tells the core through `asks` each command the person runs, by its keys, the deck or a
/// click, for the usage report. The core keeps count of only some of them.
pub fn report_use(asks: mpsc::UnboundedSender<Ask>, cx: &mut App) {
    if !cx.has_global::<Keymap>() {
        return;
    }
    let keymap = cx.global_mut::<Keymap>();
    if keymap.used.is_some() {
        return;
    }
    keymap.used = Some(asks);
    cx.observe_keystrokes(|event, _, cx| {
        if let Some(action) = &event.action {
            used(action.name(), cx);
        }
    })
    .detach();
}

fn used(id: &str, cx: &App) {
    let Some(asks) = cx
        .try_global::<Keymap>()
        .and_then(|keymap| keymap.used.as_ref())
    else {
        return;
    };
    if shortcuts().iter().any(|shortcut| shortcut.id == id) {
        let _ = asks.send(Ask::Call {
            method: "commandUsed",
            params: json!({ "id": id }),
        });
    }
}

/// The command whose shortcut `keystroke` is now.
pub fn command_for(keystroke: &Keystroke, cx: &App) -> Option<String> {
    let keys = spelling(keystroke)?;
    let keymap = cx.try_global::<Keymap>()?;
    keymap
        .bound
        .iter()
        .find(|(_, bound)| **bound == keys)
        .map(|(id, _)| id.clone())
}

/// Whether running command `id` leaves Settings open, and whether all it does then is close it.
pub fn over_settings(id: &str) -> (bool, bool) {
    let keeps = OVER_MODALS.contains(&id) || KEEP_SETTINGS.contains(&id);
    (keeps, CLOSE_SETTINGS.contains(&id))
}

/// Whether `keystroke` is the keys command `id` is bound to now.
pub fn pressed(id: &str, keystroke: &Keystroke, cx: &App) -> bool {
    let bound = cx
        .try_global::<Keymap>()
        .and_then(|keymap| keymap.bound.get(id));
    bound.is_some_and(|keys| spelling(keystroke).as_ref() == Some(keys))
}

/// The keys `id` is bound to as the app shows them, such as ⌘⇧P, or none when it is unbound.
pub fn hint(id: &str, cx: &App) -> Option<SharedString> {
    label(cx.try_global::<Keymap>()?.bound.get(id)?)
}

/// Keys in the table's spelling as the app shows them, such as ⌘⇧P.
pub fn label(keys: &str) -> Option<SharedString> {
    let mut parts: Vec<&str> = keys.split('+').collect();
    let code = parts.pop()?;
    let mac = cfg!(target_os = "macos");
    let modifiers = parts.into_iter().map(|part| match (part, mac) {
        ("Meta", true) => "⌘",
        ("Ctrl", true) => "⌃",
        ("Alt", true) => "⌥",
        ("Shift", true) => "⇧",
        (part, _) => part,
    });
    let label = match code {
        "Enter" => "↵".to_owned(),
        "Escape" => "Esc".to_owned(),
        code => NAMED
            .iter()
            .find(|(named, _)| *named == code)
            .filter(|(_, key)| key.chars().count() == 1)
            .map(|(_, key)| (*key).to_owned())
            .unwrap_or_else(|| {
                code.strip_prefix("Key")
                    .or_else(|| code.strip_prefix("Digit"))
                    .or_else(|| code.strip_prefix("Arrow"))
                    .unwrap_or(code)
                    .to_owned()
            }),
    };
    let text: Vec<String> = modifiers.map(str::to_owned).chain([label]).collect();
    Some(text.join(if mac { "" } else { "+" }).into())
}

/// A key pressed, in the table's spelling, such as `Meta+Shift+KeyP`.
pub fn spelling(keystroke: &Keystroke) -> Option<String> {
    let key = keystroke.key.as_str();
    let shifted = SHIFTED.iter().find(|(_, typed)| *typed == key);
    let code = match key.chars().collect::<Vec<_>>().as_slice() {
        [letter] if letter.is_ascii_alphabetic() => format!("Key{}", letter.to_ascii_uppercase()),
        [digit] if digit.is_ascii_digit() => format!("Digit{digit}"),
        _ if key.starts_with('f') && key[1..].parse::<u8>().is_ok() => key.to_uppercase(),
        _ => NAMED
            .iter()
            .chain(shifted)
            .find(|(_, named)| *named == key)?
            .0
            .to_owned(),
    };
    let modifiers = &keystroke.modifiers;
    let held = [
        (modifiers.platform, "Meta"),
        (modifiers.control, "Ctrl"),
        (modifiers.alt, "Alt"),
        (modifiers.shift || shifted.is_some(), "Shift"),
    ];
    let parts: Vec<&str> = held
        .into_iter()
        .filter(|(down, _)| *down)
        .map(|(_, name)| name)
        .chain([code.as_str()])
        .collect();
    Some(parts.join("+"))
}

/// Shift alone only changes what a key types, so a shortcut needs one of the others.
pub fn has_modifier(keys: &str) -> bool {
    keys.split('+')
        .any(|part| matches!(part, "Meta" | "Ctrl" | "Alt"))
}

/// What the system or every text field already does with these keys, when that is taken.
pub fn reserved(keys: &str) -> Option<&'static str> {
    let reserved: &[(&str, &str)] = match cfg!(target_os = "macos") {
        true => &[
            ("Meta+KeyQ", "Quit"),
            ("Meta+KeyH", "Hide"),
            ("Meta+Alt+KeyH", "Hide Others"),
            ("Meta+KeyM", "Minimize"),
            ("Meta+Tab", "the app switcher"),
            ("Meta+Space", "Spotlight"),
            ("Meta+KeyC", "Copy"),
            ("Meta+KeyV", "Paste"),
            ("Meta+KeyX", "Cut"),
            ("Meta+KeyA", "Select All"),
            ("Meta+KeyZ", "Undo"),
            ("Meta+Shift+KeyZ", "Redo"),
        ],
        false => &[
            ("Alt+Tab", "the window switcher"),
            ("Alt+F4", "closing the window"),
            ("Ctrl+Shift+KeyC", "Copy"),
            ("Ctrl+Shift+KeyV", "Paste"),
        ],
    };
    reserved
        .iter()
        .find(|(taken, _)| *taken == keys)
        .map(|(_, what)| *what)
}

/// A binding in the table's spelling, `Meta+Shift+KeyP`, as GPUI writes it, `cmd-shift-p`.
/// Keys in the table's spelling, such as Meta+Shift+BracketLeft, as GPUI spells them.
/// Shift with a symbol or digit reaches GPUI as the character it types, Shift+] as `}`.
pub(crate) fn keystroke(keys: &str) -> Option<String> {
    let held: Vec<&str> = keys.split('+').collect();
    let (code, modifiers) = held.split_last()?;
    let typed = SHIFTED
        .iter()
        .find(|(named, _)| named == code)
        .filter(|_| modifiers.contains(&"Shift"))
        .map(|(_, typed)| (*typed).to_owned());
    let keep_shift = typed.is_none();
    let parts: Option<Vec<String>> = modifiers
        .iter()
        .filter(|part| keep_shift || **part != "Shift")
        .map(|part| match *part {
            "Meta" => Some("cmd".to_owned()),
            "Ctrl" => Some("ctrl".to_owned()),
            "Alt" => Some("alt".to_owned()),
            "Shift" => Some("shift".to_owned()),
            _ => None,
        })
        .chain([typed.or_else(|| key(code))])
        .collect();
    Some(parts?.join("-"))
}

fn key(code: &str) -> Option<String> {
    if let Some(letter) = code.strip_prefix("Key") {
        return Some(letter.to_lowercase());
    }
    if let Some(digit) = code.strip_prefix("Digit") {
        return Some(digit.to_owned());
    }
    if let Some(number) = code.strip_prefix('F').filter(|n| n.parse::<u8>().is_ok()) {
        return Some(format!("f{number}"));
    }
    if code == "NumpadEnter" {
        return Some("enter".to_owned());
    }
    NAMED
        .iter()
        .find(|(named, _)| *named == code)
        .map(|(_, key)| (*key).to_owned())
}

/// Keys the table names by what they are, beside what GPUI calls them.
const NAMED: &[(&str, &str)] = &[
    ("Backquote", "`"),
    ("Backslash", "\\"),
    ("BracketLeft", "["),
    ("BracketRight", "]"),
    ("Comma", ","),
    ("Equal", "="),
    ("Minus", "-"),
    ("Period", "."),
    ("Quote", "'"),
    ("Semicolon", ";"),
    ("Slash", "/"),
    ("Enter", "enter"),
    ("Escape", "escape"),
    ("Tab", "tab"),
    ("Space", "space"),
    ("Backspace", "backspace"),
    ("Delete", "delete"),
    ("ArrowLeft", "left"),
    ("ArrowRight", "right"),
    ("ArrowUp", "up"),
    ("ArrowDown", "down"),
    ("Home", "home"),
    ("End", "end"),
    ("PageUp", "pageup"),
    ("PageDown", "pagedown"),
];

/// What a key types with Shift held, which is how some layouts report it.
const SHIFTED: &[(&str, &str)] = &[
    ("Backquote", "~"),
    ("Digit1", "!"),
    ("Digit2", "@"),
    ("Digit3", "#"),
    ("Digit4", "$"),
    ("Digit5", "%"),
    ("Digit6", "^"),
    ("Digit7", "&"),
    ("Digit8", "*"),
    ("Digit9", "("),
    ("Digit0", ")"),
    ("Minus", "_"),
    ("Equal", "+"),
    ("BracketLeft", "{"),
    ("BracketRight", "}"),
    ("Backslash", "|"),
    ("Semicolon", ":"),
    ("Quote", "\""),
    ("Comma", "<"),
    ("Period", ">"),
    ("Slash", "?"),
];

/// The menu bar, as today's app has it.
/// macOS's own About panel, with the name and version the bundle carries.
fn show_about() {
    #[cfg(target_os = "macos")]
    {
        use objc2::MainThreadMarker;
        use objc2_app_kit::NSApplication;
        if let Some(main) = MainThreadMarker::new() {
            NSApplication::sharedApplication(main).orderFrontStandardAboutPanel(None);
        }
    }
}

fn menus() -> Vec<Menu> {
    vec![
        Menu {
            name: "Sikemux".into(),
            items: vec![
                MenuItem::action("About Sikemux", About),
                MenuItem::separator(),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Hide Sikemux", Hide),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::separator(),
                MenuItem::action("Quit Sikemux", Quit),
                MenuItem::action("Quit and Stop Everything", QuitAndStopEverything),
            ],
            disabled: false,
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Undo", Undo, OsAction::Undo),
                MenuItem::os_action("Redo", Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", Cut, OsAction::Cut),
                MenuItem::os_action("Copy", Copy, OsAction::Copy),
                MenuItem::os_action("Paste", Paste, OsAction::Paste),
                MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
            ],
            disabled: false,
        },
        Menu {
            name: "View".into(),
            items: vec![MenuItem::action("Enter Full Screen", ToggleFullScreen)],
            disabled: false,
        },
        Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action("Minimize", Minimize),
                MenuItem::action("Zoom", Zoom),
            ],
            disabled: false,
        },
    ]
}
