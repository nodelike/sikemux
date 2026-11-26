//! Shared Browser Profile helpers, actions, and menus reused across Spaces, Desk, Playbook,
//! and Settings.

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{App, Context, SharedString, WeakEntity};
use sikemux_browser::{ProfileStore, TabProfile};
use sikemux_calls::browser_profiles::{
    BrowserProfileDeleteParams, BrowserProfileSetGlobalParams, BrowserProfileSetProjectParams,
    BrowserProfileSetSpaceParams, BrowserProfileUpsertParams,
};
use sikemux_calls::call;
use sikemux_view::browser_profiles::{
    default_profile, ensure_profiles, global_default, BrowserProfile, ProfileAccess, ProfileSource,
    ResolvedBrowserProfile,
};

use crate::core::Target;
use crate::icons::Icon;
use crate::toast;
use crate::ui;
use crate::workbench::Workbench;

pub fn source_hint(resolved: &ResolvedBrowserProfile) -> SharedString {
    match resolved.source {
        ProfileSource::AgentOverride => match resolved.source_label.as_deref() {
            Some(who) if !who.is_empty() => format!("{who} override").into(),
            _ => t!("agent override").into(),
        },
        ProfileSource::TabOverride => t!("tab override").into(),
        ProfileSource::Playbook => t!("Playbook").into(),
        ProfileSource::Space => match resolved.source_label.as_deref() {
            Some(space) if !space.is_empty() => format!("from Space: {space}").into(),
            _ => t!("from Space").into(),
        },
        ProfileSource::Global => t!("global default").into(),
    }
}

pub fn source_short(resolved: &ResolvedBrowserProfile) -> SharedString {
    match resolved.source {
        ProfileSource::AgentOverride => match resolved.source_label.as_deref() {
            Some(who) if !who.is_empty() => format!("{who} override").into(),
            _ => t!("agent override").into(),
        },
        ProfileSource::TabOverride => t!("tab override").into(),
        ProfileSource::Playbook => t!("Playbook").into(),
        ProfileSource::Space => t!("from Space").into(),
        ProfileSource::Global => t!("global default").into(),
    }
}

pub type OnProfileSaved = Box<dyn FnOnce(&mut Workbench, BrowserProfile, &mut Context<Workbench>)>;

pub fn tab_profile_for(
    profile: &BrowserProfile,
    is_override: bool,
    override_by: Option<String>,
) -> TabProfile {
    TabProfile {
        profile_id: Some(profile.id.clone()),
        store: ProfileStore::for_profile(&profile.id, profile.is_default_store),
        is_override,
        override_by,
    }
}

impl Workbench {
    pub fn browser_profiles(&self) -> Vec<BrowserProfile> {
        ensure_profiles(&self.preferences().browser_profiles)
    }

    pub fn global_default_browser_profile(&self) -> BrowserProfile {
        let prefs = self.preferences();
        let profiles = ensure_profiles(&prefs.browser_profiles);
        global_default(&profiles, prefs.default_browser_profile.as_deref())
            .cloned()
            .unwrap_or_else(default_profile)
    }

    pub fn browser_profile_by_id(&self, id: &str) -> Option<BrowserProfile> {
        self.browser_profiles()
            .into_iter()
            .find(|profile| profile.id == id)
    }

    /// The project-inherited profile (`Playbook -> Space -> Global`) for `agent_id`, read from the
    /// core's resolved `ProjectInfo::browser_profile`.
    pub fn inherited_profile_for_agent(&self, agent_id: &str) -> ResolvedBrowserProfile {
        let resolved = self.this_view().and_then(|view| {
            let project_id = view
                .workspace
                .agents
                .iter()
                .find(|agent| agent.id == agent_id)
                .map(|agent| agent.project.as_str())?;
            view.workspace
                .projects
                .iter()
                .find(|project| project.id == project_id)
                .and_then(|project| project.browser_profile.clone())
        });
        resolved.unwrap_or_else(|| ResolvedBrowserProfile {
            profile: self.global_default_browser_profile(),
            source: ProfileSource::Global,
            source_label: None,
        })
    }

    pub fn default_tab_profile_for_agent(&self, agent_id: &str) -> TabProfile {
        let inherited = self.inherited_profile_for_agent(agent_id);
        tab_profile_for(&inherited.profile, false, None)
    }

    /// `(effective, inherited)` resolved profiles for a browser tab on `agent_id`'s desk.
    pub fn resolved_tab_profile(
        &self,
        agent_id: &str,
        tab: &sikemux_browser::BrowserTab,
    ) -> (ResolvedBrowserProfile, ResolvedBrowserProfile) {
        let inherited = self.inherited_profile_for_agent(agent_id);
        let effective = if let Some(profile_id) = tab.profile_id.as_deref() {
            if let Some(profile) = self.browser_profile_by_id(profile_id) {
                if tab.profile_override {
                    let source = if tab.profile_override_by.is_some() {
                        ProfileSource::AgentOverride
                    } else {
                        ProfileSource::TabOverride
                    };
                    ResolvedBrowserProfile {
                        profile,
                        source,
                        source_label: tab.profile_override_by.clone(),
                    }
                } else if profile.id == inherited.profile.id {
                    inherited.clone()
                } else {
                    ResolvedBrowserProfile {
                        profile,
                        source: ProfileSource::TabOverride,
                        source_label: None,
                    }
                }
            } else {
                inherited.clone()
            }
        } else {
            inherited.clone()
        };
        (effective, inherited)
    }

    pub fn set_space_browser_profile(
        &mut self,
        space_id: String,
        profile_id: Option<String>,
        create_matching: bool,
        cx: &mut Context<Self>,
    ) {
        let space_name = self
            .this_view()
            .and_then(|view| view.workspace.spaces.iter().find(|s| s.id == space_id))
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let asked = Target::here(&self.asks).request_typed::<call::BrowserProfileSetSpace>(
            BrowserProfileSetSpaceParams {
                space_id,
                profile_id,
                create_matching,
            },
        );
        cx.spawn(async move |bench, cx| match asked.await {
            Ok(answer) => {
                let message = match (answer.created, answer.profile) {
                    (true, Some(profile)) => {
                        format!(
                            "Profile “{}” made · Space {} opens tabs in it",
                            profile.name, space_name
                        )
                    }
                    (false, Some(profile)) => {
                        format!("Space {} → {}", space_name, profile.name)
                    }
                    (_, None) => format!("Space {} uses the global default", space_name),
                };
                let _ = bench.update(cx, |bench, cx| bench.notify(toast::success(message), cx));
            }
            Err(error) => {
                let _ = bench.update(cx, |bench, cx| bench.notify(toast::error(error), cx));
            }
        })
        .detach();
    }

    pub fn set_project_browser_profile(
        &mut self,
        project_path: String,
        project_name: String,
        profile_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let profile_name = profile_id
            .as_deref()
            .and_then(|id| self.browser_profile_by_id(id))
            .map(|profile| profile.name);
        let asked = Target::here(&self.asks).request_typed::<call::BrowserProfileSetProject>(
            BrowserProfileSetProjectParams {
                project: project_path,
                profile_id,
            },
        );
        cx.spawn(async move |bench, cx| match asked.await {
            Ok(()) => {
                let message = match profile_name {
                    Some(name) => format!("{project_name} pinned to {name}"),
                    None => format!("{project_name} follows its space"),
                };
                let _ = bench.update(cx, |bench, cx| bench.notify(toast::success(message), cx));
            }
            Err(error) => {
                let _ = bench.update(cx, |bench, cx| bench.notify(toast::error(error), cx));
            }
        })
        .detach();
    }

    pub fn set_global_browser_profile(&mut self, profile_id: String, cx: &mut Context<Self>) {
        let name = self
            .browser_profile_by_id(&profile_id)
            .map(|profile| profile.name)
            .unwrap_or_default();
        let asked = Target::here(&self.asks).request_typed::<call::BrowserProfileSetGlobal>(
            BrowserProfileSetGlobalParams { id: profile_id },
        );
        cx.spawn(async move |bench, cx| match asked.await {
            Ok(_) => {
                let _ = bench.update(cx, |bench, cx| {
                    bench.notify(toast::success(format!("{name} is the global default")), cx)
                });
            }
            Err(error) => {
                let _ = bench.update(cx, |bench, cx| bench.notify(toast::error(error), cx));
            }
        })
        .detach();
    }

    pub fn upsert_browser_profile(
        &mut self,
        params: BrowserProfileUpsertParams,
        toast_message: Option<String>,
        on_done: Option<OnProfileSaved>,
        cx: &mut Context<Self>,
    ) {
        let asked = Target::here(&self.asks).request_typed::<call::BrowserProfileUpsert>(params);
        cx.spawn(async move |bench, cx| match asked.await {
            Ok(answer) => {
                let _ = bench.update(cx, |bench, cx| {
                    if let Some(msg) = toast_message {
                        bench.notify(toast::success(msg), cx);
                    }
                    if let Some(on_done) = on_done {
                        on_done(bench, answer.profile, cx);
                    }
                });
            }
            Err(error) => {
                let _ = bench.update(cx, |bench, cx| bench.notify(toast::error(error), cx));
            }
        })
        .detach();
    }

    pub fn clear_browser_profile_data(&mut self, profile_id: &str, cx: &mut Context<Self>) {
        let Some(profile) = self.browser_profile_by_id(profile_id) else {
            return;
        };
        let store = ProfileStore::for_profile(&profile.id, profile.is_default_store);
        if let Some(engine) = self.desks.read(cx).engine.clone() {
            engine.clear_profile_data(store);
            self.notify(
                toast::success(format!("Site data cleared for {}", profile.name)),
                cx,
            );
        }
    }

    pub fn delete_browser_profile(&mut self, profile_id: String, cx: &mut Context<Self>) {
        let Some(profile) = self.browser_profile_by_id(&profile_id) else {
            return;
        };
        let store = ProfileStore::for_profile(&profile.id, profile.is_default_store);
        let name = profile.name.clone();
        let engine = self.desks.read(cx).engine.clone();
        let asked = Target::here(&self.asks).request_typed::<call::BrowserProfileDelete>(
            BrowserProfileDeleteParams {
                id: profile_id.clone(),
            },
        );
        cx.spawn(async move |bench, cx| match asked.await {
            Ok(answer) => {
                if let Some(engine) = engine {
                    let fallback_profile = answer
                        .profiles
                        .iter()
                        .find(|p| p.id == answer.global_default_id)
                        .or_else(|| answer.profiles.first())
                        .cloned()
                        .unwrap_or_else(default_profile);
                    let fallback = tab_profile_for(&fallback_profile, false, None);
                    engine.migrate_tabs_from_profile(&profile_id, fallback);
                    engine.delete_profile_store(store);
                }
                let _ = bench.update(cx, |bench, cx| {
                    bench.notify(
                        toast::success(format!("“{name}” deleted · Spaces and Playbooks kept")),
                        cx,
                    )
                });
            }
            Err(error) => {
                let _ = bench.update(cx, |bench, cx| bench.notify(toast::error(error), cx));
            }
        })
        .detach();
    }

    pub fn switch_tab_to_profile(
        &mut self,
        agent_id: String,
        tab_id: String,
        profile_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(engine) = self.desks.read(cx).engine.clone() else {
            return;
        };
        let inherited = self.inherited_profile_for_agent(&agent_id);
        let (target_profile, is_override) = match profile_id.as_deref() {
            Some(id) => match self.browser_profile_by_id(id) {
                Some(profile) => (profile.clone(), profile.id != inherited.profile.id),
                None => return,
            },
            None => (inherited.profile.clone(), false),
        };
        let tab_profile = tab_profile_for(&target_profile, is_override, None);
        let name = target_profile.name.clone();
        match engine.switch_tab_profile(&agent_id, &tab_id, tab_profile) {
            Ok(()) => self.notify(toast::success(format!("Tab reloads as {name}")), cx),
            Err(error) => self.notify(toast::error(error.to_string()), cx),
        }
    }

    pub fn open_tab_in_named_profile(
        &mut self,
        agent_id: String,
        profile_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some(engine) = self.desks.read(cx).engine.clone() else {
            return;
        };
        let Some(profile) = self.browser_profile_by_id(&profile_id) else {
            return;
        };
        let inherited = self.inherited_profile_for_agent(&agent_id);
        let tab_profile = tab_profile_for(&profile, profile.id != inherited.profile.id, None);
        let name = profile.name.clone();
        let agent = agent_id.clone();
        self.desks.update(cx, |desks, cx| {
            desks.address_wanted = Some(agent.clone());
            cx.notify();
        });
        let this = cx.weak_entity();
        let toast_label = format!("New tab in {name}");
        crate::desk::spawn(async move {
            match engine
                .open_tab_in_profile(&agent_id, None, None, true, false, tab_profile)
                .await
            {
                Ok(tab_id) => engine.keep(&agent_id, &tab_id),
                Err(error) => eprintln!("sikemux: could not open tab in {name}: {error}"),
            }
        });
        toast::show(&this, toast::success(toast_label), cx);
    }
}

/// Shared menu items for assigning a Space's default Browser Profile (Option A).
/// Reused in both the rail Space context menu (`Browser Profile ▸`) and the `SpaceEditor` popover.
pub fn space_profile_menu(
    mut menu: PopupMenu,
    bench: WeakEntity<Workbench>,
    space_id: String,
    space_name: String,
    current_profile_id: Option<String>,
    cx: &App,
) -> PopupMenu {
    let Some(bench_entity) = bench.upgrade() else {
        return menu;
    };
    let read = bench_entity.read(cx);
    let profiles = read.browser_profiles();
    let global = read.global_default_browser_profile();
    let spaces = read
        .this_view()
        .map(|view| view.workspace.spaces.clone())
        .unwrap_or_default();

    let exists = profiles
        .iter()
        .any(|profile| profile.name.eq_ignore_ascii_case(&space_name));

    if !exists && !space_name.trim().is_empty() {
        let (bench_create, sid) = (bench.clone(), space_id.clone());
        menu = menu
            .item(
                PopupMenuItem::new(format!("Create “{space_name}” profile"))
                    .icon(Icon::Plus)
                    .on_click(move |_, _, cx| {
                        let _ = bench_create.update(cx, |bench, cx| {
                            bench.set_space_browser_profile(sid.clone(), None, true, cx);
                        });
                    }),
            )
            .separator();
    }

    for profile in &profiles {
        let other_spaces: Vec<&str> = spaces
            .iter()
            .filter(|s| {
                s.id != space_id && s.browser_profile.as_deref() == Some(profile.id.as_str())
            })
            .map(|s| s.name.as_str())
            .collect();
        let chosen = current_profile_id.as_deref() == Some(profile.id.as_str());
        let (bench_pick, sid, pid) = (bench.clone(), space_id.clone(), profile.id.clone());
        let row = ui::MenuRow::new(profile.name.clone())
            .chosen(chosen)
            .when(!other_spaces.is_empty(), |row| {
                row.hint(other_spaces.join(", "))
            });
        menu = menu.item(row.item(cx).on_click(move |_, _, cx| {
            let _ = bench_pick.update(cx, |bench, cx| {
                bench.set_space_browser_profile(sid.clone(), Some(pid.clone()), false, cx);
            });
        }));
    }

    let (bench_clear, sid) = (bench, space_id);
    menu.separator().item(
        ui::MenuRow::new(t!("No default"))
            .chosen(current_profile_id.is_none())
            .hint(format!("uses {}", global.name))
            .item(cx)
            .on_click(move |_, _, cx| {
                let _ = bench_clear.update(cx, |bench, cx| {
                    bench.set_space_browser_profile(sid.clone(), None, false, cx);
                });
            }),
    )
}

pub fn access_label(access: ProfileAccess) -> &'static str {
    match access {
        ProfileAccess::Standard => "Standard",
        ProfileAccess::AskOutsideProject => "Ask outside project",
        ProfileAccess::ManualOnly => "Manual only",
    }
}
