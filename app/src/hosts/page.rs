//! The Hosts page: every host on the account at a glance, and one host's detail beside them.

use gpui_kit::{div, AnyElement, Context, IntoElement, ParentElement, Styled, Window};
use sikemux_view::layout::{OpenTab, PaneKind};

use crate::theme::Tokens;
use crate::ui;
use crate::workbench::Workbench;

pub fn pane(
    _bench: &Workbench,
    _chosen: &str,
    _window: &Window,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    ui::pane(cx)
        .child(div().p_4().text_color(t.text_secondary).child("Hosts"))
        .into_any_element()
}

impl Workbench {
    /// Opens the Hosts page in the strip in front, on `host`'s detail when one is named.
    pub fn open_hosts(&mut self, host: Option<String>) {
        if let Some(strip) = self.strip() {
            let target = host.unwrap_or_default();
            self.open_in_front(OpenTab::new(strip.at(), PaneKind::Hosts, target));
        }
    }
}
