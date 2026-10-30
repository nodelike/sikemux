//! Keys as the agents name them: `Enter`, `Shift+Tab`, `Meta+a`. Each platform turns a
//! parsed chord into its own key events.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    /// The command key: ⌘ on macOS, Control on Linux.
    pub meta: bool,
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Named {
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    Space,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Named(Named),
    Char(char),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chord {
    pub modifiers: Modifiers,
    pub key: Key,
}

pub fn parse(name: &str) -> Result<Chord, String> {
    let mut modifiers = Modifiers::default();
    let mut rest = name;
    while let Some((modifier, tail)) = rest.split_once('+').filter(|(_, tail)| !tail.is_empty()) {
        let held = match modifier.to_ascii_lowercase().as_str() {
            "meta" | "cmd" | "command" => &mut modifiers.meta,
            "control" | "ctrl" => &mut modifiers.control,
            "alt" | "option" => &mut modifiers.alt,
            "shift" => &mut modifiers.shift,
            _ => return Err(format!("unknown modifier \"{modifier}\" in \"{name}\"")),
        };
        *held = true;
        rest = tail;
    }
    let named = match rest {
        "Enter" | "Return" => Some(Named::Enter),
        "Tab" => Some(Named::Tab),
        "Escape" | "Esc" => Some(Named::Escape),
        "Backspace" => Some(Named::Backspace),
        "Delete" => Some(Named::Delete),
        "Space" | " " => Some(Named::Space),
        "ArrowUp" => Some(Named::ArrowUp),
        "ArrowDown" => Some(Named::ArrowDown),
        "ArrowLeft" => Some(Named::ArrowLeft),
        "ArrowRight" => Some(Named::ArrowRight),
        "Home" => Some(Named::Home),
        "End" => Some(Named::End),
        "PageUp" => Some(Named::PageUp),
        "PageDown" => Some(Named::PageDown),
        _ => None,
    };
    let key = match named {
        Some(named) => Key::Named(named),
        None => {
            let mut chars = rest.chars();
            match (chars.next(), chars.next()) {
                (Some(character), None) => Key::Char(character),
                _ => return Err(format!("unknown key \"{rest}\"; use a single character or a name like Enter, Tab, Escape, ArrowDown")),
            }
        }
    };
    Ok(Chord { modifiers, key })
}
