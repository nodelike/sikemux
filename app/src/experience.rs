//! The screens over the whole window: the welcome on a first run, the release notes
//! once a new release runs, and runtime diagnostics when asked for. The core says when
//! the first two are due.

use gpui_kit::{div, AnyElement, Context, IntoElement, Window};
use serde_json::Value;
use sikemux_wire::protocol::Preferences;

use crate::workbench::Workbench;
use crate::{diagnostics, welcome, whats_new};

pub enum Experience {
    Welcome(welcome::Welcome),
    WhatsNew(whats_new::WhatsNew),
    Diagnostics(diagnostics::Diagnostics),
}

impl Experience {
    /// Whether the release sky is behind it, which keeps moving while it is up.
    fn has_sky(&self) -> bool {
        !matches!(self, Experience::Diagnostics(_))
    }
}

/// Which screens this run has already shown on its own, so neither comes back once closed.
#[derive(Default)]
pub struct Shown {
    welcome: bool,
    news: bool,
}

/// A first run opens the welcome.
pub fn preferences_read(
    bench: &mut Workbench,
    preferences: &Preferences,
    cx: &mut Context<Workbench>,
) {
    if preferences.onboarding_complete || bench.shown.welcome || bench.overlays.experience.is_some()
    {
        return;
    }
    bench.shown.welcome = true;
    open_welcome(bench, cx);
}

/// A release the person has not read the notes of opens them, once the welcome is done.
pub fn whats_new_read(bench: &mut Workbench, value: &Value, cx: &mut Context<Workbench>) {
    if value["unseen"] != true || bench.shown.news || bench.overlays.experience.is_some() {
        return;
    }
    bench.shown.news = true;
    open_whats_new(bench, cx);
}

pub fn open_welcome(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    close_shown(bench, cx);
    bench.overlays.experience = Some(Experience::Welcome(welcome::Welcome::new(cx)));
    welcome::opened(bench, cx);
    crate::ui::grain::sky_shown(true, cx);
    redraw(bench, cx);
}

pub fn open_whats_new(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    close_shown(bench, cx);
    bench.overlays.experience = Some(Experience::WhatsNew(whats_new::WhatsNew::default()));
    whats_new::opened(bench, cx);
    crate::ui::grain::sky_shown(true, cx);
    redraw(bench, cx);
}

pub fn open_diagnostics(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    close_shown(bench, cx);
    bench.overlays.experience = Some(Experience::Diagnostics(diagnostics::Diagnostics::default()));
    diagnostics::opened(bench, cx);
    redraw(bench, cx);
}

/// Closes the screen up, counting the welcome done or the release's notes read.
pub fn close(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let Some(shown) = bench.overlays.experience.take() else {
        return;
    };
    let sky = shown.has_sky();
    match shown {
        Experience::Welcome(_) => {
            crate::settings::set(bench, "onboardingComplete", true);
        }
        Experience::WhatsNew(news) => {
            if let Some(version) = news.running() {
                crate::settings::set(bench, "lastSeenVersion", version);
            }
        }
        Experience::Diagnostics(_) => {}
    }
    if sky {
        crate::ui::grain::sky_shown(false, cx);
    }
    redraw(bench, cx);
}

/// Swaps one screen for the other without counting the first as finished.
fn close_shown(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    if bench
        .overlays
        .experience
        .take()
        .is_some_and(|shown| shown.has_sky())
    {
        crate::ui::grain::sky_shown(false, cx);
    }
}

/// Opening or closing one mounts or drops the part over the shell, which only the shell decides.
fn redraw(_: &Workbench, cx: &mut Context<Workbench>) {
    cx.notify();
}

/// What the part over the shell draws.
pub fn draw(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) -> AnyElement {
    let this = cx.entity();
    match &bench.overlays.experience {
        Some(Experience::Welcome(_)) => welcome::render(bench, &this, window, cx),
        Some(Experience::WhatsNew(_)) => {
            whats_new::take_keys(bench, window, cx);
            whats_new::render(bench, &this, window, cx)
        }
        Some(Experience::Diagnostics(_)) => {
            diagnostics::take_keys(bench, window, cx);
            diagnostics::render(bench, &this, cx)
        }
        None => div().into_any_element(),
    }
}
