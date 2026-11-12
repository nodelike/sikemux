//! What the person does with the mods of the host in front: open one, pin it to a project, turn
//! it off, remove it, or ask an agent to make or change one.

use gpui_kit::{Context, Window};
use sikemux_core::server::mods::ModInfo;
use sikemux_view::layout::OpenTab;
use sikemux_view::layout::PaneKind;
use tokio::sync::oneshot;

use crate::icons::Icon;
use crate::palette::{Mark, Row, Spec};
use crate::workbench::{Strip, Workbench};
use sikemux_core::server::call;
use sikemux_core::server::chat::calls::PromptChatParams;
use sikemux_core::server::mods::{change_prompt, make_prompt};
use sikemux_core::server::mods::{ModParams, OffParams, PinModParams};

/// The palette's row that asks an agent for a new mod.
const MAKE: &str = "mods:make";
/// How the palette's rows that copy an example start.
const EXAMPLE: &str = "example:";

/// The icon a mod's `mod.json` names, or the one every mod without one wears.
pub fn icon(shown: &ModInfo) -> Icon {
    shown
        .icon
        .as_deref()
        .and_then(Icon::named)
        .unwrap_or(Icon::Dashboard)
}

impl Workbench {
    /// The mods that show here, to open one, and a row that asks an agent for a new one.
    pub fn toggle_mods_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut rows: Vec<Row> = self
            .mods_here()
            .map(|shown| {
                Row::new(shown.id.clone(), shown.name.clone())
                    .meta(
                        shown
                            .badge
                            .clone()
                            .unwrap_or_else(|| shown.description.clone()),
                    )
                    .mark(Mark::Icon(icon(shown)))
            })
            .collect();
        let examples = self
            .feeds
            .in_front()
            .mods
            .iter()
            .flat_map(|list| &list.examples);
        rows.extend(examples.filter(|example| !example.added).map(|example| {
            Row::new(format!("{EXAMPLE}{}", example.id), example.name.clone())
                .meta(t!("An example to copy and change"))
                .mark(Mark::Icon(Icon::Plus))
        }));
        rows.push(
            Row::new(MAKE, t!("Make a mod with an agent"))
                .meta(t!("Say what it should show"))
                .mark(Mark::Icon(Icon::Sparkle)),
        );
        let spec = Spec::new(
            "palette.mods",
            t!("open a mod…"),
            rows,
            |row, bench, _, cx| {
                let name = row.title.to_string();
                match row.key.as_ref() {
                    MAKE => bench.ask_for_mod(None, cx),
                    key => match key.strip_prefix(EXAMPLE) {
                        Some(id) => bench.add_example_mod(id, &name, cx),
                        None => bench.open_mod(key, &name),
                    },
                }
            },
        )
        .width(440.);
        self.toggle_palette(spec, window, cx);
    }

    /// The mods the host in front lists that are on and show in the project in front.
    pub fn mods_here(&self) -> impl Iterator<Item = &ModInfo> {
        let project = self
            .project()
            .map(|project| project.path.to_string_lossy().into_owned());
        self.feeds
            .in_front()
            .mods
            .iter()
            .flat_map(|list| list.mods.iter())
            .filter(move |shown| {
                !shown.off
                    && shown
                        .project
                        .as_ref()
                        .is_none_or(|pinned| Some(pinned) == project.as_ref())
            })
    }

    pub fn open_mod(&self, id: &str, name: &str) {
        if let Some(strip) = self.strip() {
            self.open_in_front(OpenTab::new(strip.at(), PaneKind::Mod, id).titled(name));
        }
    }

    pub fn add_example_mod(&self, id: &str, name: &str, cx: &mut Context<Self>) {
        let added = self
            .target()
            .request_typed::<call::AddExampleMod>(ModParams { id: id.to_owned() });
        let (id, name) = (id.to_owned(), name.to_owned());
        self.report(
            added,
            t!("The example mod was not added"),
            move |bench, _, _| bench.open_mod(&id, &name),
            cx,
        );
    }

    /// Shows `id` only in the project in front, or again in every project when it is pinned.
    pub fn pin_mod(&self, id: &str, pinned: bool) {
        let project = (!pinned)
            .then(|| self.project())
            .flatten()
            .map(|project| project.path.to_string_lossy().into_owned());
        self.target().call_typed::<call::PinMod>(PinModParams {
            id: id.to_owned(),
            project,
        });
    }

    pub fn turn_mod_off(&self, id: &str, off: bool) {
        self.target().call_typed::<call::TurnModOff>(OffParams {
            id: id.to_owned(),
            off,
        });
    }

    pub fn remove_mod(&mut self, shown: &ModInfo, cx: &mut Context<Self>) {
        let id = shown.id.clone();
        self.confirm(
            t!("Remove {name}?", name = shown.name),
            t!("Its folder moves to the Trash."),
            t!("Remove"),
            true,
            move |bench, yes, _, _| {
                if yes {
                    bench
                        .target()
                        .call_typed::<call::RemoveMod>(ModParams { id: id.clone() });
                }
            },
            cx,
        );
    }

    /// Asks what the person wants of a new mod, or of `editing`, and hands it to an agent in the
    /// project in front.
    pub fn ask_for_mod(&mut self, editing: Option<ModInfo>, cx: &mut Context<Self>) {
        let title = match &editing {
            Some(shown) => t!("What should change in {name}?", name = shown.name),
            None => t!("What should the new mod show?").to_owned(),
        };
        self.prompt(
            title,
            String::new(),
            move |bench, asked, window, cx| {
                bench.hand_mod_to_agent(&asked, editing.as_ref(), window, cx)
            },
            cx,
        );
    }

    fn hand_mod_to_agent(
        &mut self,
        asked: &str,
        editing: Option<&ModInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let asked = asked.trim();
        if asked.is_empty() {
            return;
        }
        let Some(Strip::Project(project)) = self.strip() else {
            return;
        };
        let Some(launcher) = self
            .launcher()
            .map(|launcher| self.new_chat_launcher(launcher))
        else {
            return self.toggle_agent_palette(window, cx);
        };
        let folder = self
            .feeds
            .in_front()
            .mods
            .as_ref()
            .map(|list| list.folder.display().to_string())
            .unwrap_or_else(|| "~/.config/sikemux/mods".to_owned());
        let prompt = match editing {
            Some(shown) => change_prompt(&shown.name, &folder, &shown.id, asked),
            None => make_prompt(asked, &folder),
        };
        let (started, agent) = oneshot::channel();
        let target = self.target();
        target.start_chat(launcher, project, None, Some(started));
        cx.spawn(async move |_, _| {
            if let Ok(agent) = agent.await {
                target.call_typed::<call::PromptChat>(PromptChatParams::new(agent, prompt));
            }
        })
        .detach();
    }
}
