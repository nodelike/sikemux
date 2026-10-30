//! Offers to move a skill an agent saved in its own folder into Sikemux, where every agent gets it.

use gpui_kit::component::WindowExt;
use gpui_kit::Context;
use serde_json::{json, Value};

use crate::core;
use crate::toast;
use crate::workbench::Workbench;

impl Workbench {
    /// Says, once each, which skills the agents saved in their own folders since the core last
    /// looked, with a way to move each into Sikemux.
    pub fn offer_skills(&mut self, added: &Value, cx: &mut Context<Self>) {
        for skill in added.as_array().into_iter().flatten() {
            let text = |key: &str| skill[key].as_str().unwrap_or_default().to_owned();
            let (agent, name, path) = (text("agent"), text("name"), text("path"));
            if path.is_empty() || !self.skills_offered.insert(path.clone()) {
                continue;
            }
            let asks = self.asks.clone();
            let note = toast::info(format!(
                "{agent} saved the skill {name} in its own folder, so only {agent} has it, outside Sikemux too."
            ))
            .action(move |_, _, cx| {
                let (note, asks, path, name) =
                    (cx.entity(), asks.clone(), path.clone(), name.clone());
                gpui_kit::component::button::Button::new("move-skill")
                    .label("Move into Sikemux")
                    .on_click(move |_, window, cx| {
                        let moved = core::request(&asks, "adoptSkill", json!({ "path": path }));
                        let name = name.clone();
                        window
                            .spawn(cx, async move |cx| {
                                let said = match moved.await {
                                    Ok(_) => toast::success(format!(
                                        "{name} is in Sikemux now, for every agent's next session."
                                    )),
                                    Err(error) => toast::error(format!("Move {name}: {error}")),
                                };
                                let _ = cx.update(|window, cx| window.push_notification(said, cx));
                            })
                            .detach();
                        note.update(cx, |note, cx| note.dismiss(window, cx));
                    })
            });
            self.notify(note, cx);
        }
    }
}
