//! Asks once, when a chat's turn has just finished in front of the person, for a star on
//! GitHub, which is how other people find Sikemux.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::component::notification::Notification;
use gpui_kit::{App, Context};
use sikemux_calls::call;
use sikemux_calls::usage::Step;
use sikemux_view::prefs::PreferenceChange;

use crate::core::Target;
use crate::screens::whats_new::REPO;
use crate::toast;
use crate::workbench::Workbench;

/// Set once the ask is shown, so a core that cannot save `star_asked` does not bring it back.
static ASKED: AtomicBool = AtomicBool::new(false);

pub fn turn_finished(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let preferences = bench.preferences();
    if preferences.star_asked
        || !preferences.onboarding_complete
        || cx.active_window().is_none()
        || ASKED.swap(true, Ordering::Relaxed)
    {
        return;
    }
    let here = Target::here(&bench.asks);
    here.call_quietly::<call::SetPreference>(PreferenceChange::StarAsked(true));
    here.call_quietly::<call::FirstRunStep>(Step::StarAsked);
    let ask = note(move |_| here.call_quietly::<call::FirstRunStep>(Step::StarOpened));
    bench.notify(ask, cx);
}

/// The ask itself, whose Star button opens the repository after `starred`.
pub fn note(starred: impl Fn(&mut App) + 'static) -> Notification {
    toast::acting(
        t!("Enjoying Sikemux? A star on GitHub helps others find it."),
        t!("Star"),
        move |_, cx| {
            starred(cx);
            cx.open_url(REPO);
        },
    )
    .autohide(false)
}
