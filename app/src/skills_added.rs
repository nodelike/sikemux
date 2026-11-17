//! Offers to move a skill an agent saved in its own folder into Sikemux, where every agent gets it.

use gpui_kit::component::WindowExt;
use gpui_kit::Context;
use sikemux_calls::call;
use sikemux_calls::skill_library::GlobalSkillAdded;
use sikemux_calls::skill_library::Located;

use crate::core;
use crate::toast;
use crate::workbench::Workbench;

impl Workbench {
    /// Says, once each, which skills the agents saved in their own folders since the core last
    /// looked, with a way to move each into Sikemux.
    pub fn offer_skills(&mut self, added: Vec<GlobalSkillAdded>, cx: &mut Context<Self>) {
        for GlobalSkillAdded { agent, name, path } in added {
            if !self.skills_offered.insert(path.clone()) {
                continue;
            }
            let asks = self.asks.clone();
            let note = toast::info(t!("{agent} saved the skill {name} in its own folder, so only {agent} has it, outside Sikemux too.", agent = agent, name = name))
            .action(move |_, _, cx| {
                let (note, asks, path, name) =
                    (cx.entity(), asks.clone(), path.clone(), name.clone());
                gpui_kit::component::button::Button::new("move-skill")
                    .label(t!("Move into Sikemux"))
                    .on_click(move |_, window, cx| {
                        let moved = core::Target::here(&asks)
                            .request_typed::<call::AdoptSkill>(Located { path: path.clone() });
                        let name = name.clone();
                        window
                            .spawn(cx, async move |cx| {
                                let said = match moved.await {
                                    Ok(_) => toast::success(t!("{name} is in Sikemux now, for every agent's next session.", name = name)),
                                    Err(error) => toast::error(t!("Move {name}: {error}", name = name, error = error)),
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
