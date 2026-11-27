//! Shared Browser Profile helpers, actions, strictly flat picker, and Modal M1 reused across
//! Desk, Playbook, Spaces, and Settings.

use std::rc::Rc;

use gpui_kit::base::Selectable;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, svg, Anchor, AnyElement, App, AppContext, AsyncApp, Context, Div, ElementId, Entity,
    FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement, Radians, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Transformation, WeakEntity,
    Window,
};
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
use crate::theme::{
    self, radius_1, radius_2, Tokens, SPACE_1, SPACE_1H, SPACE_2, TEXT_2XS, TEXT_2XS_H, TEXT_3XS_H,
    TEXT_MD, TEXT_SM, TEXT_XS, TEXT_XS_H,
};
use crate::toast;
use crate::ui::{self, dialog, Button, Clicks, IconButton, Variant};
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

pub type OnProfileSaved = Box<dyn FnOnce(WeakEntity<Workbench>, BrowserProfile, &mut AsyncApp)>;

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

pub fn access_label(access: ProfileAccess) -> &'static str {
    match access {
        ProfileAccess::Standard => "Standard",
        ProfileAccess::AskOutsideProject => "Ask outside project",
        ProfileAccess::ManualOnly => "Manual only",
    }
}

pub fn access_description(access: ProfileAccess) -> &'static str {
    match access {
        ProfileAccess::Standard => t!("Agents pick it on their own."),
        ProfileAccess::AskOutsideProject => {
            t!("Agents ask before using it outside its project.")
        }
        ProfileAccess::ManualOnly => t!("Only you pick it; agents never do."),
    }
}

pub fn profile_initial(name: &str) -> SharedString {
    name.trim()
        .chars()
        .next()
        .map(|ch| ch.to_uppercase().to_string())
        .unwrap_or_else(|| "·".to_owned())
        .into()
}

pub fn profile_mark(name: &str, size: f32, font_size: f32, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius_1())
        .border(theme::border())
        .border_color(t.border_strong)
        .text_size(theme::ui_text(font_size))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.text_primary)
        .child(profile_initial(name))
}

pub fn flat_input(input: &Entity<InputState>, height: f32, font_size: f32, cx: &App) -> Div {
    let t = Tokens::get(cx).clone();
    div()
        .w_full()
        .h(px(height))
        .px(px(10.))
        .flex()
        .items_center()
        .rounded(radius_2())
        .border(theme::border())
        .border_color(t.border)
        .hover(move |field| field.border_color(t.border_strong))
        .text_size(theme::ui_text(font_size))
        .text_color(t.text_primary)
        .child(Input::new(input).appearance(false))
}

type OnAccessPick = Rc<dyn Fn(ProfileAccess, &mut Window, &mut App)>;

pub fn access_segmented(
    id_prefix: impl Into<SharedString>,
    current: ProfileAccess,
    on_pick: impl Fn(ProfileAccess, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Div {
    let t = Tokens::get(cx).clone();
    let prefix = id_prefix.into();
    let on_pick: OnAccessPick = Rc::new(on_pick);
    let policies = [
        ProfileAccess::Standard,
        ProfileAccess::AskOutsideProject,
        ProfileAccess::ManualOnly,
    ];
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(2.))
        .p(px(2.))
        .rounded(radius_2())
        .border(theme::border())
        .border_color(t.border)
        .children(policies.into_iter().map(move |policy| {
            let on = current == policy;
            let label = access_label(policy);
            let on_pick = on_pick.clone();
            let t = t.clone();
            div()
                .id(SharedString::from(format!("{prefix}-{label}")))
                .flex_1()
                .h(px(24.))
                .px(SPACE_2)
                .flex()
                .items_center()
                .justify_center()
                .whitespace_nowrap()
                .rounded(radius_1())
                .cursor_pointer()
                .text_size(theme::ui_text(TEXT_XS))
                .when(on, |btn| {
                    btn.bg(t.surface_chosen)
                        .text_color(t.text_primary)
                        .font_weight(FontWeight::MEDIUM)
                })
                .when(!on, |btn| {
                    btn.text_color(t.text_tertiary)
                        .hover(move |btn| btn.bg(t.surface_hover).text_color(t.text_primary))
                })
                .clicks(move |_, window, cx| on_pick(policy, window, cx))
                .child(label)
        }))
}

#[derive(Clone, Debug)]
pub enum NewProfileTarget {
    SwitchTab {
        agent_id: String,
        tab_id: String,
    },
    Playbook {
        folder: String,
        project_name: String,
    },
    Space {
        space_id: String,
        space_name: String,
    },
    Settings,
}

impl NewProfileTarget {
    fn primary_label(&self) -> &'static str {
        match self {
            Self::SwitchTab { .. } => t!("Create & switch"),
            Self::Playbook { .. } | Self::Space { .. } => t!("Create & use"),
            Self::Settings => t!("Create profile"),
        }
    }

    fn enter_hint(&self) -> &'static str {
        match self {
            Self::SwitchTab { .. } => t!("creates and switches"),
            Self::Playbook { .. } | Self::Space { .. } => t!("creates and uses it"),
            Self::Settings => t!("creates"),
        }
    }
}

pub struct AddBrowserProfile {
    target: NewProfileTarget,
    pub(crate) name_input: Entity<InputState>,
    pub(crate) note_input: Entity<InputState>,
    pub(crate) access: ProfileAccess,
    pub(crate) agent_access_open: bool,
    submitting: bool,
    _subs: Vec<Subscription>,
}

impl AddBrowserProfile {
    fn new(
        target: NewProfileTarget,
        initial_name: Option<String>,
        window: &mut Window,
        cx: &mut Context<Workbench>,
    ) -> Self {
        let prefill = initial_name.unwrap_or_default();
        let has_prefill = !prefill.trim().is_empty();
        let name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("Work, Client staging…"))
                .default_value(prefill)
        });
        let note_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("Jira, staging, team SaaS")));
        name_input.update(cx, |input, cx| {
            input.focus(window, cx);
            if has_prefill {
                input.select_all(window, cx);
            }
        });
        let name_sub = cx.subscribe_in(
            &name_input,
            window,
            |bench, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => submit_add_modal(bench, window, cx),
                _ => {}
            },
        );
        let note_sub = cx.subscribe_in(
            &note_input,
            window,
            |bench, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => submit_add_modal(bench, window, cx),
                _ => {}
            },
        );
        Self {
            target,
            name_input,
            note_input,
            access: ProfileAccess::Standard,
            agent_access_open: false,
            submitting: false,
            _subs: vec![name_sub, note_sub],
        }
    }
}

fn close_add_modal(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    bench.overlays.adding_browser_profile = None;
    cx.notify();
}

fn submit_add_modal(bench: &mut Workbench, _window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(modal) = bench.overlays.adding_browser_profile.as_ref() else {
        return;
    };
    if modal.submitting {
        return;
    }
    let name = modal.name_input.read(cx).value().trim().to_owned();
    if name.is_empty() || bench.has_browser_profile_named(&name, None) {
        return;
    }
    let description = modal.note_input.read(cx).value().trim().to_owned();
    let access = modal.access;
    let target = modal.target.clone();
    if let Some(modal) = bench.overlays.adding_browser_profile.as_mut() {
        modal.submitting = true;
    }
    cx.notify();

    let toast_message = match &target {
        NewProfileTarget::Settings => Some(format!("“{name}” created")),
        _ => None,
    };

    let params = BrowserProfileUpsertParams {
        id: None,
        name: name.clone(),
        slug: None,
        description: Some(description),
        access: Some(access),
        link_space_id: None,
    };

    let on_done: OnProfileSaved = Box::new(move |bench, created, cx| {
        let _ = bench.update(cx, |bench, cx| {
            bench.overlays.adding_browser_profile = None;
            match target {
                NewProfileTarget::SwitchTab { agent_id, tab_id } => {
                    bench.switch_tab_to_resolved_profile(
                        agent_id,
                        tab_id,
                        created,
                        Some(format!("“{name}” created · tab switched")),
                        cx,
                    );
                }
                NewProfileTarget::Playbook {
                    folder,
                    project_name,
                } => {
                    bench.set_project_browser_profile_with_toast(
                        folder,
                        project_name.clone(),
                        Some(created.id),
                        Some(format!("“{name}” created · {project_name} uses it")),
                        cx,
                    );
                }
                NewProfileTarget::Space {
                    space_id,
                    space_name,
                } => {
                    bench.set_space_browser_profile_with_toast(
                        space_id,
                        Some(created.id),
                        false,
                        Some(format!("“{name}” created · Space {space_name} uses it")),
                        cx,
                    );
                }
                NewProfileTarget::Settings => {
                    crate::settings::select_browser_profile(bench, created.id, cx);
                }
            }
            cx.notify();
        });
    });

    bench.upsert_browser_profile(params, toast_message, Some(on_done), cx);
}

/// Strictly flat Modal M1 for creating a new browser profile from any surface.
pub fn add_modal_view(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let modal = bench.overlays.adding_browser_profile.as_ref()?;
    let t = Tokens::get(cx).clone();
    let this = cx.entity();
    let typed_name = modal.name_input.read(cx).value().to_string();
    let trimmed = typed_name.trim();
    let duplicate = !trimmed.is_empty() && bench.has_browser_profile_named(trimmed, None);
    let can_create = !trimmed.is_empty() && !duplicate && !modal.submitting;
    let access = modal.access;
    let expanded = modal.agent_access_open;

    let close_x = this.clone();
    let close_cancel = this.clone();
    let toggle_more = this.clone();
    let pick_access = this.clone();
    let submit_click = this.clone();
    let submit_enter = this.clone();
    let close_esc = this.clone();

    let header = div()
        .pl(px(18.))
        .pr(px(14.))
        .pt(px(14.))
        .flex()
        .items_center()
        .justify_between()
        .child(
            ui::text(t!("New browser profile"), TEXT_MD, FontWeight::SEMIBOLD)
                .text_color(t.text_primary),
        )
        .child(
            IconButton::new("bp-modal-close", Icon::Close, t!("Close"))
                .compact()
                .on_click(move |_, _, cx| close_x.update(cx, close_add_modal)),
        );

    let chevron = ui::icon(Icon::Chevron, 12.)
        .text_color(t.icon_quiet)
        .when(expanded, |chev| {
            chev.with_transformation(Transformation::rotate(Radians(std::f32::consts::FRAC_PI_2)))
        });

    let body = div()
        .px(px(18.))
        .pt(px(14.))
        .pb(px(16.))
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(SPACE_1H)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(profile_mark(&typed_name, 30., TEXT_SM, cx))
                        .child(div().flex_1().min_w_0().child(flat_input(
                            &modal.name_input,
                            36.,
                            TEXT_MD,
                            cx,
                        ))),
                )
                .when(duplicate, |col| {
                    col.child(
                        div()
                            .pl(px(40.))
                            .text_size(theme::ui_text(TEXT_2XS_H))
                            .text_color(t.danger)
                            .child(t!("A profile with this name already exists.")),
                    )
                }),
        )
        .child(
            div()
                .id("bp-modal-agent-access")
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .cursor_pointer()
                .text_size(theme::ui_text(TEXT_XS))
                .text_color(t.text_tertiary)
                .hover(move |row| row.text_color(t.text_primary))
                .clicks(move |_, _, cx| {
                    toggle_more.update(cx, |bench, cx| {
                        if let Some(modal) = bench.overlays.adding_browser_profile.as_mut() {
                            modal.agent_access_open = !modal.agent_access_open;
                            cx.notify();
                        }
                    });
                })
                .child(chevron)
                .child(t!("Agent access"))
                .when(!expanded, |row| {
                    row.child(
                        div()
                            .text_color(t.text_disabled)
                            .child(format!("· {}", access_label(access))),
                    )
                }),
        )
        .when(expanded, |body| {
            body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_1H)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                ui::text(t!("Note for agents"), TEXT_XS, FontWeight::MEDIUM)
                                    .text_color(t.text_secondary),
                            )
                            .child(
                                div()
                                    .text_size(theme::ui_text(TEXT_2XS_H))
                                    .text_color(t.text_disabled)
                                    .child(t!("optional")),
                            ),
                    )
                    .child(flat_input(&modal.note_input, 30., TEXT_SM, cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_1H)
                    .child(
                        ui::text(t!("Agent access"), TEXT_XS, FontWeight::MEDIUM)
                            .text_color(t.text_secondary),
                    )
                    .child(access_segmented(
                        "bp-modal-access",
                        access,
                        move |policy, _, cx| {
                            pick_access.update(cx, |bench, cx| {
                                if let Some(modal) = bench.overlays.adding_browser_profile.as_mut()
                                {
                                    modal.access = policy;
                                    cx.notify();
                                }
                            });
                        },
                        cx,
                    ))
                    .child(
                        div()
                            .text_size(theme::ui_text(TEXT_2XS_H))
                            .text_color(t.text_tertiary)
                            .child(access_description(access)),
                    ),
            )
        });

    let footer = div()
        .pl(px(18.))
        .pr(px(14.))
        .py(px(12.))
        .border_t(theme::border())
        .border_color(t.line_soft)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .text_size(theme::ui_text(TEXT_2XS_H))
                .text_color(t.text_tertiary)
                .child(ui::kbd("↵", cx))
                .child(modal.target.enter_hint()),
        )
        .child(div().flex_1())
        .child(
            Button::new("bp-modal-cancel", t!("Cancel"), Variant::Ghost)
                .on_click(move |_, _, cx| close_cancel.update(cx, close_add_modal)),
        )
        .child(
            Button::new(
                "bp-modal-create",
                modal.target.primary_label(),
                Variant::Primary,
            )
            .disabled(!can_create)
            .on_click(move |_, window, cx| {
                submit_click.update(cx, |bench, cx| submit_add_modal(bench, window, cx))
            }),
        );

    let content = div()
        .flex()
        .flex_col()
        .child(header)
        .child(body)
        .child(footer);

    let yes: dialog::Answer = Rc::new(move |window, cx| {
        submit_enter.update(cx, |bench, cx| submit_add_modal(bench, window, cx))
    });
    let no: dialog::Answer = Rc::new(move |_, cx| close_esc.update(cx, close_add_modal));

    Some(
        dialog::floating(px(400.), content, yes, no)
            .flat()
            .into_any_element(),
    )
}

impl Workbench {
    pub fn open_new_browser_profile_modal(
        &mut self,
        target: NewProfileTarget,
        initial_name: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.overlays.adding_browser_profile =
            Some(AddBrowserProfile::new(target, initial_name, window, cx));
        cx.notify();
    }

    pub fn browser_profiles(&self) -> Vec<BrowserProfile> {
        ensure_profiles(&self.preferences().browser_profiles)
    }

    pub fn has_browser_profile_named(&self, name: &str, exclude_id: Option<&str>) -> bool {
        let needle = name.trim();
        self.browser_profiles().iter().any(|profile| {
            exclude_id.is_none_or(|id| profile.id != id)
                && profile.name.trim().eq_ignore_ascii_case(needle)
        })
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
        self.set_space_browser_profile_with_toast(space_id, profile_id, create_matching, None, cx);
    }

    fn set_space_browser_profile_with_toast(
        &mut self,
        space_id: String,
        profile_id: Option<String>,
        create_matching: bool,
        toast_override: Option<String>,
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
                let message =
                    toast_override.unwrap_or_else(|| match (answer.created, answer.profile) {
                        (true, Some(profile)) => {
                            format!(
                                "Profile “{}” made · Space {} opens tabs in it",
                                profile.name, space_name
                            )
                        }
                        (false, Some(profile)) => {
                            format!("Space {} → {}", space_name, profile.name)
                        }
                        (_, None) => format!("Space {} follows the global default", space_name),
                    });
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
        self.set_project_browser_profile_with_toast(
            project_path,
            project_name,
            profile_id,
            None,
            cx,
        );
    }

    fn set_project_browser_profile_with_toast(
        &mut self,
        project_path: String,
        project_name: String,
        profile_id: Option<String>,
        toast_override: Option<String>,
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
                let message = toast_override.unwrap_or_else(|| match profile_name {
                    Some(name) => format!("{project_name} pinned to {name}"),
                    None => format!("{project_name} follows its space"),
                });
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
                });
                if let Some(on_done) = on_done {
                    on_done(bench, answer.profile, cx);
                }
            }
            Err(error) => {
                let _ = bench.update(cx, |bench, cx| {
                    if let Some(modal) = bench.overlays.adding_browser_profile.as_mut() {
                        modal.submitting = false;
                    }
                    bench.notify(toast::error(error), cx);
                    cx.notify();
                });
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
        let inherited = self.inherited_profile_for_agent(&agent_id);
        let target_profile = match profile_id.as_deref() {
            Some(id) => match self.browser_profile_by_id(id) {
                Some(profile) => profile,
                None => return,
            },
            None => inherited.profile,
        };
        self.switch_tab_to_resolved_profile(agent_id, tab_id, target_profile, None, cx);
    }

    fn switch_tab_to_resolved_profile(
        &mut self,
        agent_id: String,
        tab_id: String,
        target_profile: BrowserProfile,
        toast_override: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(engine) = self.desks.read(cx).engine.clone() else {
            return;
        };
        let inherited = self.inherited_profile_for_agent(&agent_id);
        let is_override = target_profile.id != inherited.profile.id;
        let tab_profile = tab_profile_for(&target_profile, is_override, None);
        let name = target_profile.name.clone();
        match engine.switch_tab_profile(&agent_id, &tab_id, tab_profile) {
            Ok(()) => self.notify(
                toast::success(toast_override.unwrap_or_else(|| format!("Tab reloads as {name}"))),
                cx,
            ),
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

/// Which surface opened the shared flat browser profile picker.
#[derive(Clone)]
pub enum PickerMode {
    DeskTab {
        agent_id: String,
        tab_id: String,
        active_profile_id: String,
        inherited_profile_id: Option<String>,
    },
    Playbook {
        folder: String,
        project_name: String,
        pinned_profile_id: Option<String>,
        inherited_name: String,
    },
    Space {
        space_id: String,
        space_name: String,
        current_profile_id: Option<String>,
    },
    GlobalDefault {
        current_global_id: String,
    },
}

/// Single strictly flat Aura Noir browser profile picker menu reused across Desk, Playbook,
/// Spaces, and Settings.
pub fn profile_picker_menu(
    mut menu: PopupMenu,
    bench: WeakEntity<Workbench>,
    mode: PickerMode,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(bench_entity) = bench.upgrade() else {
        return menu;
    };
    let read = bench_entity.read(cx);
    let profiles = read.browser_profiles();
    let global = read.global_default_browser_profile();
    let menu_weak = cx.entity().downgrade();
    menu = menu.min_w(px(236.));

    match &mode {
        PickerMode::Playbook {
            folder,
            project_name,
            pinned_profile_id,
            inherited_name,
        } => {
            let chosen = pinned_profile_id.is_none();
            let (bench_follow, folder, project_name, inherited_name) = (
                bench.clone(),
                folder.clone(),
                project_name.clone(),
                inherited_name.clone(),
            );
            menu = menu
                .item(
                    flat_follow_item(
                        "bp-pick-follow-space",
                        t!("Follow space"),
                        inherited_name,
                        chosen,
                    )
                    .on_click(move |_, _, cx| {
                        let _ = bench_follow.update(cx, |bench, cx| {
                            bench.set_project_browser_profile(
                                folder.clone(),
                                project_name.clone(),
                                None,
                                cx,
                            );
                        });
                    }),
                )
                .separator();
        }
        PickerMode::Space {
            space_id,
            current_profile_id,
            ..
        } => {
            let chosen = current_profile_id.is_none();
            let (bench_follow, space_id, global_name) =
                (bench.clone(), space_id.clone(), global.name.clone());
            menu = menu
                .item(
                    flat_follow_item(
                        "bp-pick-follow-default",
                        t!("Follow default"),
                        global_name,
                        chosen,
                    )
                    .on_click(move |_, _, cx| {
                        let _ = bench_follow.update(cx, |bench, cx| {
                            bench.set_space_browser_profile(space_id.clone(), None, false, cx);
                        });
                    }),
                )
                .separator();
        }
        PickerMode::DeskTab { .. } | PickerMode::GlobalDefault { .. } => {}
    }

    for profile in &profiles {
        let chosen = match &mode {
            PickerMode::DeskTab {
                active_profile_id, ..
            } => active_profile_id == &profile.id,
            PickerMode::Playbook {
                pinned_profile_id, ..
            } => pinned_profile_id.as_deref() == Some(profile.id.as_str()),
            PickerMode::Space {
                current_profile_id, ..
            } => current_profile_id.as_deref() == Some(profile.id.as_str()),
            PickerMode::GlobalDefault { current_global_id } => current_global_id == &profile.id,
        };
        let show_inherited = match &mode {
            PickerMode::DeskTab {
                inherited_profile_id,
                ..
            } => inherited_profile_id.as_deref() == Some(profile.id.as_str()),
            _ => false,
        };
        let new_tab_action = match &mode {
            PickerMode::DeskTab { agent_id, .. } => Some((
                bench.clone(),
                agent_id.clone(),
                profile.id.clone(),
                menu_weak.clone(),
            )),
            _ => None,
        };

        let name: SharedString = profile.name.clone().into();
        let row_group: SharedString = format!("bp-pick-row-{}", profile.id).into();
        let row_id: SharedString = format!("bp-pick-item-{}", profile.id).into();
        let btn_id: SharedString = format!("bp-pick-newtab-{}", profile.id).into();
        let new_tab_tip: SharedString = format!("New tab in {}", profile.name).into();

        let item = PopupMenuItem::element(move |_, cx| {
            let t = Tokens::get(cx).clone();
            let side = px(8.);
            div()
                .id(row_id.clone())
                .group(row_group.clone())
                .relative()
                .flex_1()
                .min_w_0()
                .h(px(theme::dense(28.)))
                .flex()
                .items_center()
                .gap(px(9.))
                .when(chosen, |row| {
                    row.child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(-side)
                            .right(-side)
                            .rounded(radius_2())
                            .bg(t.surface_chosen),
                    )
                })
                .child(ui::icon(Icon::User, 13.).text_color(t.icon_quiet))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(t.text_primary)
                        .child(name.clone()),
                )
                .when(show_inherited, |row| {
                    row.child(
                        div()
                            .flex_none()
                            .whitespace_nowrap()
                            .text_size(theme::ui_text(TEXT_2XS))
                            .text_color(t.text_tertiary)
                            .child(t!("inherited")),
                    )
                })
                .when_some(
                    new_tab_action.clone(),
                    |row, (bench_new, aid, pid, dismiss)| {
                        row.child(
                            div()
                                .id(btn_id.clone())
                                .flex_none()
                                .size(px(20.))
                                .rounded(radius_1())
                                .flex()
                                .items_center()
                                .justify_center()
                                .opacity(0.)
                                .group_hover(row_group.clone(), |btn| btn.opacity(1.))
                                .text_color(t.icon_quiet)
                                .hover(move |btn| {
                                    btn.bg(t.surface_active).text_color(t.text_primary)
                                })
                                .tooltip(ui::tooltip(new_tab_tip.clone()))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .clicks(move |_, _, cx| {
                                    cx.stop_propagation();
                                    let _ = bench_new.update(cx, |bench, cx| {
                                        bench.open_tab_in_named_profile(
                                            aid.clone(),
                                            pid.clone(),
                                            cx,
                                        );
                                    });
                                    let _ = dismiss.update(cx, |_, cx| {
                                        cx.emit(gpui_kit::DismissEvent);
                                    });
                                })
                                .child(ui::icon(Icon::SquarePlus, 12.)),
                        )
                    },
                )
                .child(
                    div()
                        .flex_none()
                        .w(px(13.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(chosen, |slot| {
                            slot.child(ui::icon(Icon::Check, 13.).text_color(t.text_primary))
                        }),
                )
        });

        let bench_pick = bench.clone();
        let pid = profile.id.clone();
        let mode_pick = mode.clone();
        let item = item.on_click(move |_, _, cx| {
            let pid = pid.clone();
            match &mode_pick {
                PickerMode::DeskTab {
                    agent_id, tab_id, ..
                } => {
                    let (aid, tid) = (agent_id.clone(), tab_id.clone());
                    let _ = bench_pick.update(cx, |bench, cx| {
                        bench.switch_tab_to_profile(aid, tid, Some(pid), cx);
                    });
                }
                PickerMode::Playbook {
                    folder,
                    project_name,
                    ..
                } => {
                    let (folder, name) = (folder.clone(), project_name.clone());
                    let _ = bench_pick.update(cx, |bench, cx| {
                        bench.set_project_browser_profile(folder, name, Some(pid), cx);
                    });
                }
                PickerMode::Space { space_id, .. } => {
                    let sid = space_id.clone();
                    let _ = bench_pick.update(cx, |bench, cx| {
                        bench.set_space_browser_profile(sid, Some(pid), false, cx);
                    });
                }
                PickerMode::GlobalDefault { .. } => {
                    let _ = bench_pick.update(cx, |bench, cx| {
                        bench.set_global_browser_profile(pid, cx);
                    });
                }
            }
        });
        menu = menu.item(item);
    }

    let (modal_target, initial_name, show_manage) = match &mode {
        PickerMode::DeskTab {
            agent_id, tab_id, ..
        } => (
            NewProfileTarget::SwitchTab {
                agent_id: agent_id.clone(),
                tab_id: tab_id.clone(),
            },
            None,
            true,
        ),
        PickerMode::Playbook {
            folder,
            project_name,
            ..
        } => (
            NewProfileTarget::Playbook {
                folder: folder.clone(),
                project_name: project_name.clone(),
            },
            None,
            true,
        ),
        PickerMode::Space {
            space_id,
            space_name,
            ..
        } => {
            let prefill = (!space_name.trim().is_empty()
                && !read.has_browser_profile_named(space_name, None))
            .then(|| space_name.clone());
            (
                NewProfileTarget::Space {
                    space_id: space_id.clone(),
                    space_name: space_name.clone(),
                },
                prefill,
                true,
            )
        }
        PickerMode::GlobalDefault { .. } => (NewProfileTarget::Settings, None, false),
    };

    let bench_add = bench.clone();
    menu = menu.separator().item(
        PopupMenuItem::element(|_, cx| {
            let t = Tokens::get(cx);
            div()
                .flex_1()
                .min_w_0()
                .h(px(theme::dense(28.)))
                .flex()
                .items_center()
                .gap(px(9.))
                .text_color(t.text_primary)
                .child(ui::icon(Icon::Plus, 13.).text_color(t.icon_quiet))
                .child(t!("New profile…"))
        })
        .on_click(move |_, window, cx| {
            let target = modal_target.clone();
            let initial = initial_name.clone();
            let _ = bench_add.update(cx, |bench, cx| {
                bench.open_new_browser_profile_modal(target, initial, window, cx);
            });
        }),
    );

    if show_manage {
        let bench_manage = bench;
        menu = menu.item(
            PopupMenuItem::element(|_, cx| {
                let t = Tokens::get(cx);
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(theme::dense(28.)))
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .text_color(t.text_primary)
                    .child(ui::icon(Icon::Settings, 13.).text_color(t.icon_quiet))
                    .child(t!("Manage profiles…"))
            })
            .on_click(move |_, window, cx| {
                let _ = bench_manage.update(cx, |bench, cx| {
                    crate::settings::open(
                        bench,
                        crate::settings::Page::BrowserProfiles,
                        window,
                        cx,
                    );
                });
            }),
        );
    }

    menu
}

fn flat_follow_item(
    id: &'static str,
    lead: &'static str,
    inherited_name: String,
    chosen: bool,
) -> PopupMenuItem {
    let suffix: SharedString = format!("· {inherited_name}").into();
    PopupMenuItem::element(move |_, cx| {
        let t = Tokens::get(cx);
        let side = px(8.);
        div()
            .id(id)
            .relative()
            .flex_1()
            .min_w_0()
            .h(px(theme::dense(28.)))
            .flex()
            .items_center()
            .gap(px(6.))
            .when(chosen, |row| {
                row.child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(-side)
                        .right(-side)
                        .rounded(radius_2())
                        .bg(t.surface_chosen),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(SPACE_1)
                    .overflow_hidden()
                    .child(div().flex_none().text_color(t.text_primary).child(lead))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(t.text_tertiary)
                            .child(suffix.clone()),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(13.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(chosen, |slot| {
                        slot.child(ui::icon(Icon::Check, 13.).text_color(t.text_primary))
                    }),
            )
    })
}

/// Strictly flat field trigger for the shared browser profile picker (used in Playbook,
/// Settings Global default, and SpaceEditor).
pub fn profile_picker_trigger(
    id: impl Into<ElementId>,
    bench: &Workbench,
    bench_weak: WeakEntity<Workbench>,
    mode: PickerMode,
    anchor: Anchor,
    compact: bool,
) -> impl IntoElement {
    let (primary, muted) = trigger_labels(bench, &mode);
    let trigger = FlatPickerTrigger {
        primary,
        muted,
        compact,
        open: false,
    };
    ui::menu_button(id, anchor, trigger, move |menu, _, cx| {
        profile_picker_menu(menu, bench_weak.clone(), mode.clone(), cx)
    })
}

fn trigger_labels(bench: &Workbench, mode: &PickerMode) -> (SharedString, Option<SharedString>) {
    match mode {
        PickerMode::Playbook {
            pinned_profile_id,
            inherited_name,
            ..
        } => {
            if let Some(profile) = pinned_profile_id
                .as_deref()
                .and_then(|id| bench.browser_profile_by_id(id))
            {
                (profile.name.into(), None)
            } else {
                (
                    t!("Follow space").into(),
                    Some(format!("· {inherited_name}").into()),
                )
            }
        }
        PickerMode::Space {
            current_profile_id, ..
        } => {
            if let Some(profile) = current_profile_id
                .as_deref()
                .and_then(|id| bench.browser_profile_by_id(id))
            {
                (profile.name.into(), None)
            } else {
                let global = bench.global_default_browser_profile();
                (
                    t!("Follow default").into(),
                    Some(format!("· {}", global.name).into()),
                )
            }
        }
        PickerMode::GlobalDefault { current_global_id } => {
            let name = bench
                .browser_profile_by_id(current_global_id)
                .map(|p| p.name)
                .unwrap_or_else(|| bench.global_default_browser_profile().name);
            (name.into(), None)
        }
        PickerMode::DeskTab {
            active_profile_id, ..
        } => {
            let name = bench
                .browser_profile_by_id(active_profile_id)
                .map(|p| p.name)
                .unwrap_or_else(|| bench.global_default_browser_profile().name);
            (name.into(), None)
        }
    }
}

#[derive(IntoElement)]
struct FlatPickerTrigger {
    primary: SharedString,
    muted: Option<SharedString>,
    compact: bool,
    open: bool,
}

impl Selectable for FlatPickerTrigger {
    fn selected(mut self, selected: bool) -> Self {
        self.open = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.open
    }
}

impl RenderOnce for FlatPickerTrigger {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let border_color = if self.open {
            t.border_selected
        } else {
            t.border
        };
        let hover_border = if self.open {
            t.border_selected
        } else {
            t.border_strong
        };
        div()
            .when(self.compact, |el| {
                el.h(px(24.))
                    .max_w(px(140.))
                    .pl(px(7.))
                    .pr(px(5.))
                    .gap(px(5.))
                    .rounded(radius_1())
                    .text_size(theme::ui_text(TEXT_3XS_H))
            })
            .when(!self.compact, |el| {
                el.w(px(220.))
                    .h(px(28.))
                    .pl(px(10.))
                    .pr(px(8.))
                    .gap(SPACE_2)
                    .rounded(radius_2())
                    .text_size(theme::ui_text(TEXT_XS_H))
            })
            .flex()
            .items_center()
            .border(theme::border())
            .border_color(border_color)
            .hover(move |el| el.border_color(hover_border))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(SPACE_1)
                    .overflow_hidden()
                    .child(
                        div()
                            .when(self.muted.is_none(), |el| el.flex_1().min_w_0().truncate())
                            .when(self.muted.is_some(), |el| el.flex_none())
                            .text_color(if self.compact && self.muted.is_some() {
                                t.text_tertiary
                            } else {
                                t.text_primary
                            })
                            .child(self.primary),
                    )
                    .when_some(self.muted, |row, muted| {
                        row.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(t.text_tertiary)
                                .child(muted),
                        )
                    }),
            )
            .child(
                svg()
                    .path("icons/chevron-down.svg")
                    .size(px(if self.compact { 11. } else { 13. }))
                    .flex_none()
                    .text_color(t.icon_quiet),
            )
    }
}
