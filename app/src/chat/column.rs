//! How wide a chat's column of messages grows, as the person set it.

use gpui_kit::{px, App, Global, Pixels};
use sikemux_view::prefs::ChatWidth;

struct Chosen(ChatWidth);

impl Global for Chosen {}

/// Sets the width every chat's column grows to; false when it already was.
pub fn set(width: ChatWidth, cx: &mut App) -> bool {
    if cx
        .try_global::<Chosen>()
        .is_some_and(|kept| kept.0 == width)
    {
        return false;
    }
    cx.set_global(Chosen(width));
    true
}

/// The widest a message row grows, the room at its sides included.
pub fn rows(cx: &App) -> Pixels {
    px(
        match cx
            .try_global::<Chosen>()
            .map_or(ChatWidth::default(), |kept| kept.0)
        {
            ChatWidth::Comfortable => 800.,
            ChatWidth::Wide => 1200.,
            ChatWidth::Full => 100_000.,
        },
    )
}

/// The widest the composer grows: a little past the rows, so its edge frames them.
pub fn composer(cx: &App) -> Pixels {
    rows(cx) + px(20.)
}
