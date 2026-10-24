//! The plugins built into the app, each switched on or off.

use gpui_kit::component::switch::Switch;
use gpui_kit::{
    div, AnyElement, App, Context, Entity, IntoElement, ParentElement, SharedString, Window,
};
use serde_json::{json, Value};

use crate::core::Ask;
use crate::icons::Icon;
use crate::settings::rows;
use crate::theme::Tokens;
use crate::ui;
use crate::workbench::Workbench;

pub const SECTIONS: &[&str] = &["Built-in plugins"];

/// Switches `id` on or off by sending the core every plugin that is off.
fn switch(bench: &Workbench, id: &str, on: bool) {
    let mut off: Vec<String> = bench
        .read("plugins")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|plugin| plugin["enabled"].as_bool() == Some(false))
        .filter_map(|plugin| plugin["id"].as_str().map(str::to_owned))
        .filter(|known| known != id)
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
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let ink = Tokens::get(cx).text_secondary;
    let plugins: Vec<Value> = bench
        .read("plugins")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let body = match plugins.is_empty() {
        true => rows::note("No plugins in this build.", cx),
        false => div()
            .children(plugins.iter().filter_map(|plugin| {
                let id = plugin["id"].as_str()?.to_owned();
                let name = plugin["name"].as_str().unwrap_or(&id).to_owned();
                let on = plugin["enabled"].as_bool() == Some(true);
                let about: SharedString = match plugin["signedIn"].as_bool() {
                    Some(true) => format!("{id} · signed in"),
                    Some(false) => format!("{id} · not signed in"),
                    None => id.clone(),
                }
                .into();
                let mark = match id.as_str() {
                    "sikemux.github" => Icon::Github,
                    other => crate::rail::plugin_mark(other).unwrap_or(Icon::Plug),
                };
                let this = this.clone();
                let toggle = Switch::new(SharedString::from(format!("plugin-on-{id}")))
                    .checked(on)
                    .on_change(move |on: &bool, _: &mut Window, cx: &mut App| {
                        let on = *on;
                        this.update(cx, |bench, _| switch(bench, &id, on))
                    });
                Some(rows::row_with(
                    Some(ui::picture(mark, 16., ink).into_any_element()),
                    name,
                    Some(about),
                    toggle,
                    cx,
                ))
            }))
            .into_any_element(),
    };
    vec![rows::section(
        "Built-in plugins",
        None,
        Some("A plugin switched off leaves the rail, the top bar and agents' tools, and costs nothing until it is back on.".into()),
        lit("Built-in plugins"),
        body,
        cx,
    )]
}
