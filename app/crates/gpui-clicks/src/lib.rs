use gpui::{App, ClickEvent, StatefulInteractiveElement, Styled, Window};

/// Every click handler in the app goes through here, so nothing that acts on a click forgets the
/// pointing hand. GPUI's own `on_click` is banned in `clippy.toml`.
pub trait Clicks: StatefulInteractiveElement + Styled {
    /// Acts on a click and shows the pointing hand over the element.
    #[allow(clippy::disallowed_methods)]
    fn clicks(self, listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.cursor_pointer().on_click(listener)
    }

    /// Acts on a click but leaves the cursor as the element sets it: for a backdrop that dismisses,
    /// a pane body that takes focus, or a text surface with its own cursor.
    #[allow(clippy::disallowed_methods)]
    fn clicks_keep_cursor(
        self,
        listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click(listener)
    }
}

impl<E: StatefulInteractiveElement + Styled> Clicks for E {}
