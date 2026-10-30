//! The plugins built into the app, each switched on or off.

use gpui_kit::component::switch::Switch;
use gpui_kit::{div, AnyElement, App, Entity, IntoElement, ParentElement, SharedString, Window};
use serde_json::json;

use crate::core::Ask;
use crate::icons::Icon;
use crate::settings::rows;
use crate::theme::Tokens;
use crate::ui;
use crate::workbench::Workbench;

/// Switches `id` on or off by sending the core every plugin that is off.
fn switch(bench: &Workbench, id: &str, on: bool) {
    let mut off: Vec<String> = bench
        .reads
        .plugins
        .all()
        .filter(|plugin| !plugin.enabled && plugin.id != id)
        .map(|plugin| plugin.id.clone())
        .collect();
    if !on {
        off.push(id.to_owned());
    }
    let _ = bench.asks.send(Ask::Call {
        method: "pluginSetDisabled",
        params: json!({ "ids": off }),
    });
}

pub fn render(
    bench: &Workbench,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> Vec<AnyElement> {
    let ink = Tokens::get(cx).text_secondary;
    let plugins = &bench.reads.plugins;
    let body = match plugins.is_empty() {
        true => rows::note("No plugins in this build.", cx),
        false => div()
            .children(plugins.all().map(|plugin| {
                let id = plugin.id.clone();
                let name = plugin.name.clone();
                let on = plugin.enabled;
                let about: SharedString = match plugin.signed_in {
                    Some(true) => format!("{id} · signed in"),
                    Some(false) => format!("{id} · not signed in"),
                    None => id.clone(),
                }
                .into();
                let mark = id
                    .strip_prefix("sikemux.")
                    .and_then(Icon::named)
                    .unwrap_or(Icon::Plug);
                let this = this.clone();
                let toggle = Switch::new(SharedString::from(format!("plugin-on-{id}")))
                    .checked(on)
                    .on_change(move |on: &bool, _: &mut Window, cx: &mut App| {
                        let on = *on;
                        this.update(cx, |bench, _| switch(bench, &id, on))
                    });
                rows::row_with(
                    Some(ui::picture(mark, 16., ink).into_any_element()),
                    name,
                    Some(about),
                    toggle,
                    cx,
                )
            }))
            .into_any_element(),
    };
    vec![rows::section(
        "Integrations",
        None,
        Some("A plugin switched off leaves the rail, the top bar and agents' tools, and costs nothing until it is back on.".into()),
        lit("Integrations"),
        body,
        cx,
    )]
}
