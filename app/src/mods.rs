//! What the person does with the mods of the host in front: open one, pin it to a project, turn
//! it off, remove it, or ask an agent to make or change one.

use gpui_kit::{Context, Window};
use serde_json::json;
use sikemux_core::server::ModInfo;
use sikemux_view::layout::PaneKind;
use tokio::sync::oneshot;

use crate::icons::Icon;
use crate::palette::{Mark, Row, Spec};
use crate::workbench::{Strip, Workbench};

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
        let examples = self.reads.mods.iter().flat_map(|list| &list.examples);
        rows.extend(examples.filter(|example| !example.added).map(|example| {
            Row::new(format!("{EXAMPLE}{}", example.id), example.name.clone())
                .meta("An example to copy and change")
                .mark(Mark::Icon(Icon::Plus))
        }));
        rows.push(
            Row::new(MAKE, "Make a mod with an agent")
                .meta("Say what it should show")
                .mark(Mark::Icon(Icon::Sparkle)),
        );
        let spec = Spec::new(
            "palette.mods",
            "open a mod…",
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
        self.reads
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

    /// Opens the mod `id` in a tab of the strip in front, or brings the tab holding it forward.
    pub fn open_mod(&self, id: &str, name: &str) {
        if let Some(strip) = self.strip() {
            let params = json!({ "kind": PaneKind::Mod, "target": id, "title": name });
            self.call_in_front("openTab", strip.with(params));
        }
    }

    /// Copies the example `id` into the mods folder and opens it.
    pub fn add_example_mod(&self, id: &str, name: &str, cx: &mut Context<Self>) {
        let added = self.target().request("addExampleMod", json!({ "id": id }));
        let (id, name) = (id.to_owned(), name.to_owned());
        cx.spawn(async move |bench, cx| match added.await {
            Ok(_) => {
                let _ = bench.update(cx, |bench, _| bench.open_mod(&id, &name));
            }
            Err(error) => {
                let _ = bench.update(cx, |bench, cx| {
                    bench.notify(crate::toast::error(error).autohide(false), cx)
                });
            }
        })
        .detach();
    }

    /// Shows `id` only in the project in front, or again in every project when it is pinned.
    pub fn pin_mod(&self, id: &str, pinned: bool) {
        let project = (!pinned)
            .then(|| self.project())
            .flatten()
            .map(|project| project.path.to_string_lossy().into_owned());
        self.target()
            .call("pinMod", json!({ "id": id, "project": project }));
    }

    pub fn turn_mod_off(&self, id: &str, off: bool) {
        self.target()
            .call("turnModOff", json!({ "id": id, "off": off }));
    }

    pub fn remove_mod(&mut self, shown: &ModInfo, cx: &mut Context<Self>) {
        let id = shown.id.clone();
        self.confirm(
            format!("Remove {}?", shown.name),
            "Its folder moves to the Trash.",
            "Remove",
            true,
            move |bench, yes, _, _| {
                if yes {
                    bench.target().call("removeMod", json!({ "id": id }));
                }
            },
            cx,
        );
    }

    /// Asks what the person wants of a new mod, or of `editing`, and hands it to an agent in the
    /// project in front.
    pub fn ask_for_mod(&mut self, editing: Option<ModInfo>, cx: &mut Context<Self>) {
        let title = match &editing {
            Some(shown) => format!("What should change in {}?", shown.name),
            None => "What should the new mod show?".to_owned(),
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
        let Some(launcher) = self.launcher().map(|launcher| launcher.id.clone()) else {
            return self.toggle_agent_palette(window, cx);
        };
        let folder = self
            .reads
            .mods
            .as_ref()
            .map(|list| list.folder.display().to_string())
            .unwrap_or_else(|| "~/.config/sikemux/mods".to_owned());
        let prompt = match editing {
            Some(shown) => format!(
                "Change the Sikemux mod \"{}\" in {folder}/{}: {asked}\n\nFollow the sikemux-mods skill.",
                shown.name, shown.id
            ),
            None => format!(
                "Make a Sikemux mod: {asked}\n\nFollow the sikemux-mods skill: make it in a new folder of {folder}, then open it with ui_open."
            ),
        };
        let (started, agent) = oneshot::channel();
        let target = self.target();
        target.start_chat(launcher, project, None, Some(started));
        cx.spawn(async move |_, _| {
            if let Ok(agent) = agent.await {
                target.call("promptChat", json!({ "agentId": agent, "text": prompt }));
            }
        })
        .detach();
    }
}
