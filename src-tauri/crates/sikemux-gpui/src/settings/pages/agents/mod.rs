//! The agents Sikemux runs: which are installed, the accounts and profiles they
//! start on, the models they may use, their skills, and how sessions behave.

use gpui_kit::{AnyElement, Context, Entity, Window};
use serde_json::Value;

use crate::workbench::Workbench;

mod accounts;
mod device;
mod profiles;
mod providers;
mod sessions;
mod setup;
mod skills;

pub const SECTIONS: &[&str] = &[
    "Agents on this machine",
    "Launch boundary",
    "Claude accounts",
    "Codex accounts",
    "Provider profiles",
    "Coding plans",
    "Model providers",
    "Local models",
    "Helpers",
    "Skills",
    "Sessions",
    "Voice",
    "Notch",
    "iOS Simulator",
];

/// What the page keeps between frames, made the first time it shows.
pub struct Fields {
    sessions: sessions::Fields,
    device: device::Fields,
    profiles: profiles::Fields,
    providers: providers::Fields,
    skills: skills::Fields,
}

fn make(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    crate::accounts::read(bench, cx);
    let fields = Fields {
        sessions: sessions::Fields::new(bench, window, cx),
        device: device::Fields::new(bench, window, cx),
        profiles: profiles::Fields::new(window, cx),
        providers: providers::Fields::new(window, cx),
        skills: skills::Fields::new(window, cx),
    };
    if let Some(settings) = bench.settings.as_mut() {
        settings.pages.agents = Some(fields);
    }
    crate::agent_setup::read(bench, cx);
    sessions::read_names(bench, window, cx);
    device::read_simulator(bench, cx);
    providers::read(bench, cx);
    skills::read(bench, cx);
    bench.parts.redraw(&bench.parts.stage, cx);
}

fn fields_of(bench: &mut Workbench) -> Option<&mut Fields> {
    bench.settings.as_mut()?.pages.agents.as_mut()
}

/// The catalog's entry for the agent `kind`.
fn kind<'a>(bench: &'a Workbench, kind: &str) -> Option<&'a Value> {
    bench
        .read("catalog")
        .and_then(Value::as_array)
        .and_then(|kinds| kinds.iter().find(|entry| entry["type"] == kind))
}

/// What the agent is called, as the catalog names it.
fn label(bench: &Workbench, agent: &str) -> String {
    kind(bench, agent)
        .and_then(|kind| kind["label"].as_str())
        .unwrap_or(agent)
        .to_owned()
}

pub fn render(
    bench: &Workbench,
    pages: &super::State,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let Some(fields) = &pages.agents else {
        cx.defer_in(window, make);
        return Vec::new();
    };
    let mut sections = vec![
        setup::render(bench, &bench.agent_setup, lit, this, cx),
        sessions::boundary(bench, lit, this, cx),
    ];
    sections.extend(
        ["claude", "codex"].map(|provider| accounts::render(bench, provider, lit, this, cx)),
    );
    sections.push(profiles::render(bench, &fields.profiles, lit, this, cx));
    sections.extend(providers::render(
        bench,
        &fields.providers,
        lit,
        this,
        window,
        cx,
    ));
    sections.push(skills::render(&fields.skills, lit, this, cx));
    sections.push(sessions::render(bench, &fields.sessions, lit, this, cx));
    sections.extend(device::render(bench, &fields.device, lit, this, cx));
    sections.extend(accounts::dialog(bench, this, cx));
    sections.extend(crate::agent_setup::sheet(bench, this, window, cx));
    sections
}
