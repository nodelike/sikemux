//! Vercel's pane, laid out as Vercel's dashboard: the team's projects beside a project's
//! overview, deployments, environment variables and domains, and a deployment with its
//! build log, to roll back, promote, redeploy, cancel, open or hand to an agent.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, Anchor, AnyElement, App, AppContext, ClickEvent, Context, Entity, FontWeight, Hsla,
    InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement,
    Styled, Subscription, Window,
};
use serde_json::{json, Value};
use sikemux_plugin_api::SignInStatus;
use sikemux_plugin_vercel_views::{
    Deployment, Deployments, Domains, LogLine, OpenLog, Production, Project, Projects, Signed,
    Target, Variables,
};
use std::rc::Rc;

use super::{after, listen, Host, Panel, PluginPane};
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, RADIUS_2, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H,
    TEXT_2XS_H,
};
use crate::toast;
use crate::ui;

pub const REGISTRATION: super::Registration = super::Registration {
    plugin: "sikemux.vercel",
    surface: "projects",
    pane: super::boxed::<Vercel>,
    top_bar: None,
    palettes: &[],
};

/// Who is signed in to Vercel, as the plugin's `status` says.
type Status = SignInStatus<Signed>;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Section {
    #[default]
    Overview,
    Deployments,
    Environment,
    Domains,
}

const SECTIONS: [(Section, &str); 4] = [
    (Section::Overview, "Overview"),
    (Section::Deployments, "Deployments"),
    (Section::Environment, "Environment Variables"),
    (Section::Domains, "Domains"),
];

const ENVIRONMENTS: [&str; 3] = ["All Environments", "Production", "Preview"];
const STATUSES: [&str; 5] = ["All", "Ready", "Error", "Building", "Canceled"];

/// A change the person asked for, waiting for them to say yes.
struct Asking {
    title: SharedString,
    body: SharedString,
    yes: &'static str,
    method: &'static str,
    params: Value,
    done: &'static str,
}

#[derive(Default)]
pub struct Vercel {
    project: Option<String>,
    section: Section,
    deployment: Option<String>,
    environment: usize,
    status: usize,
    find: Option<Entity<InputState>>,
    token: Option<Entity<InputState>>,
    typing: Vec<Subscription>,
    asking: Option<Asking>,
    busy: bool,
}

impl Panel for Vercel {
    fn render(
        &mut self,
        host: &mut Host,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let status = match host.status(cx).read::<Status>() {
            Ok(Some(status)) => status,
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(&failure, cx),
        };
        if !status.ok || status.details.account.is_none() {
            return self.sign_in(host, &status, window, cx);
        }
        let team = status.details.team.clone().unwrap_or_default();
        let projects = host
            .view("projectsView", json!({ "team": team }), cx)
            .read::<Projects>();
        let listed = projects.as_ref().ok().and_then(Option::as_ref).cloned();
        let project = listed.as_ref().and_then(|listed| {
            listed
                .projects
                .iter()
                .find(|each| Some(&each.name) == self.project.as_ref())
                .cloned()
        });
        let find = self.find_field(window, cx);
        let words = find.read(cx).value().trim().to_lowercase();
        let sidebar = self.sidebar(
            &status,
            &team,
            listed.as_deref(),
            project.as_ref(),
            &find,
            cx,
        );
        let page = match (&projects, &project, self.deployment.clone()) {
            (Ok(None), _, _) => super::loading(cx),
            (Err(failure), _, _) => super::failure(failure, cx),
            (Ok(Some(listed)), None, _) => self.projects(listed, &words, cx),
            (_, Some(project), Some(id)) => self.deployment(host, &team, project, &id, cx),
            (_, Some(project), None) => match self.section {
                Section::Overview => self.overview(host, &team, project, cx),
                Section::Deployments => self.deployments(host, &team, project, &words, cx),
                Section::Environment => self.environment(host, &team, project, &words, cx),
                Section::Domains => self.domains(host, &team, project, cx),
            },
        };
        let title = match (&project, &self.deployment) {
            (None, _) => "Projects".to_owned(),
            (Some(project), Some(_)) => format!("{} · Deployment Details", project.name),
            (Some(project), None) => {
                let section = SECTIONS
                    .iter()
                    .find(|(section, _)| *section == self.section)
                    .map_or("", |(_, label)| label);
                format!("{} · {section}", project.name)
            }
        };
        let refresh = ui::IconButton::new("vercel-refresh", Icon::Refresh, "Read Vercel again")
            .on_click(listen::<Vercel, ClickEvent>(cx, |_, host, _, _, _| {
                host.read_again(None)
            }));
        let t = Tokens::get(cx).clone();
        let asking = self.confirm(cx);
        div()
            .size_full()
            .flex()
            .child(sidebar)
            .child(
                div()
                    .flex_1()
                    .min_w(px(320.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(40.))
                            .flex_none()
                            .px(SPACE_3H)
                            .flex()
                            .items_center()
                            .border_b_1()
                            .border_color(t.border)
                            .child(
                                ui::text(title, 13., FontWeight::SEMIBOLD)
                                    .text_color(t.text_primary),
                            )
                            .child(div().flex_1())
                            .child(refresh),
                    )
                    .child(
                        div()
                            .id("vercel-page")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p(SPACE_3H)
                            .child(page),
                    ),
            )
            .children(asking)
            .into_any_element()
    }
}

type Act = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// Still on its way: building, queued or starting.
fn underway(state: &str) -> bool {
    matches!(state, "BUILDING" | "QUEUED" | "INITIALIZING")
}

fn state_colour(state: &str, t: &Tokens) -> Hsla {
    match state {
        "READY" => t.live,
        "ERROR" => t.danger,
        state if underway(state) => t.warn,
        _ => t.text_tertiary,
    }
}

fn state_word(state: &str) -> String {
    match state {
        "" => "No deployment".to_owned(),
        state => {
            let mut chars = state.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_string() + &chars.as_str().to_lowercase()
            })
        }
    }
}

fn dot(state: &str, t: &Tokens) -> AnyElement {
    div()
        .size(px(8.))
        .flex_none()
        .rounded_full()
        .bg(state_colour(state, t))
        .into_any_element()
}

/// How long a build ran, in seconds or minutes and seconds.
fn took(ms: u64) -> String {
    let seconds = ms / 1000;
    match seconds {
        0..60 => format!("{seconds}s"),
        _ => format!("{}m {}s", seconds / 60, seconds % 60),
    }
}

fn host_of(url: &str) -> &str {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
}

/// Builds print plenty to stderr that is not a failure, so only lines that say so are marked.
fn looks_like_error(line: &LogLine) -> bool {
    let text = line.text.to_lowercase();
    line.kind == "fatal"
        || text.contains("error")
        || text.contains("failed")
        || line.text.contains("ERR!")
}

impl Vercel {
    fn find_field(
        &mut self,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> Entity<InputState> {
        if let Some(find) = &self.find {
            return find.clone();
        }
        let find = cx.new(|cx| InputState::new(window, cx).placeholder("Find…"));
        self.typing
            .push(cx.subscribe(&find, |_, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    cx.notify();
                }
            }));
        self.find = Some(find.clone());
        find
    }

    fn open(&mut self, project: Option<String>, section: Section, cx: &mut Context<PluginPane>) {
        self.project = project;
        self.section = section;
        self.deployment = None;
        cx.notify();
    }

    /// Asks the person before a change, then makes it and says so.
    fn ask(&mut self, asking: Asking, cx: &mut Context<PluginPane>) {
        self.asking = Some(asking);
        cx.notify();
    }

    fn confirm(&self, cx: &mut Context<PluginPane>) -> Option<AnyElement> {
        let asking = self.asking.as_ref()?;
        let (yes, no) = (cx.weak_entity(), cx.weak_entity());
        Some(
            ui::Confirm::new(asking.title.clone(), asking.body.clone(), asking.yes)
                .marked()
                .on_yes(move |_, cx| {
                    let _ = yes.update(cx, |pane, cx| {
                        if let Some((vercel, host)) = pane.parts::<Vercel>() {
                            vercel.go_ahead(host, cx);
                        }
                    });
                })
                .on_no(move |_, cx| {
                    let _ = no.update(cx, |pane, cx| {
                        if let Some((vercel, _)) = pane.parts::<Vercel>() {
                            vercel.asking = None;
                            cx.notify();
                        }
                    });
                })
                .into_any_element(),
        )
    }

    fn go_ahead(&mut self, host: &mut Host, cx: &mut Context<PluginPane>) {
        let Some(asking) = self.asking.take() else {
            return;
        };
        self.busy = true;
        let done = asking.done;
        let what = asking.yes;
        let call = host.call(asking.method, asking.params);
        after(
            call,
            move |vercel: &mut Vercel, host, answer, cx| {
                vercel.busy = false;
                match answer {
                    Ok(_) => host.notify(toast::success(done), cx),
                    Err(failure) => host.said(&failure.titled(what), cx),
                }
            },
            cx,
        );
        cx.notify();
    }

    fn rollback(&mut self, team: &str, deployment: &Deployment, cx: &mut Context<PluginPane>) {
        self.ask(
            Asking {
                title: "Roll back production?".into(),
                body: format!(
                    "{} ({}) starts serving production again, without building.",
                    name_of(deployment),
                    crate::ui::time::ago(deployment.created_at)
                )
                .into(),
                yes: "Roll back",
                method: "promote",
                params: json!({ "team": team, "id": deployment.id }),
                done: "Rolled production back",
            },
            cx,
        );
    }

    fn promote(&mut self, team: &str, deployment: &Deployment, cx: &mut Context<PluginPane>) {
        self.ask(
            Asking {
                title: "Promote to production?".into(),
                body: format!(
                    "Vercel builds {} for production, and it serves the production domains once it is ready.",
                    name_of(deployment)
                )
                .into(),
                yes: "Promote",
                method: "redeploy",
                params: json!({ "team": team, "id": deployment.id, "target": "production" }),
                done: "Promoting to production",
            },
            cx,
        );
    }

    fn redeploy(&mut self, team: &str, deployment: &Deployment, cx: &mut Context<PluginPane>) {
        let target = match deployment.target {
            Target::Production => "production",
            Target::Preview => "preview",
        };
        self.ask(
            Asking {
                title: "Redeploy?".into(),
                body: format!(
                    "Vercel builds {} again as a new {target} deployment.",
                    name_of(deployment)
                )
                .into(),
                yes: "Redeploy",
                method: "redeploy",
                params: json!({ "team": team, "id": deployment.id }),
                done: "Redeploying",
            },
            cx,
        );
    }

    fn cancel(&mut self, team: &str, deployment: &Deployment, cx: &mut Context<PluginPane>) {
        self.ask(
            Asking {
                title: "Cancel this deployment?".into(),
                body: format!("{} stops building.", name_of(deployment)).into(),
                yes: "Cancel deployment",
                method: "cancel",
                params: json!({ "team": team, "id": deployment.id }),
                done: "Canceled the deployment",
            },
            cx,
        );
    }

    /// The ⋯ menu of what can be done to a deployment, in the order Vercel's own menu offers it.
    fn more(
        &self,
        id: SharedString,
        label: SharedString,
        team: &str,
        deployment: &Deployment,
        current: bool,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let ready = deployment.state == "READY";
        let busy = self.busy;
        let production = deployment.target == Target::Production;
        let does = |act: fn(&mut Vercel, &str, &Deployment, &mut Context<PluginPane>),
                    cx: &mut Context<PluginPane>|
         -> Option<Act> {
            let (team, deployment) = (team.to_owned(), deployment.clone());
            Some(Rc::new(listen::<Vercel, ClickEvent>(
                cx,
                move |vercel, _, _, _, cx| act(vercel, &team, &deployment, cx),
            )))
        };
        let opens = |url: Option<String>| -> Option<Act> {
            url.map(|url| {
                Rc::new(move |_: &ClickEvent, _: &mut Window, cx: &mut App| cx.open_url(&url))
                    as Act
            })
        };
        let (inspector, url) = (deployment.inspector_url.clone(), deployment.url.clone());
        let copies = url.clone().map(|url| {
            Rc::new(listen::<Vercel, ClickEvent>(
                cx,
                move |_, host, _, _, cx| host.copy(url.clone(), "the URL", cx),
            )) as Act
        });
        let items: Vec<(&'static str, Option<Icon>, bool, Option<Act>)> = vec![
            (
                "Instant Rollback",
                Some(Icon::Rotate),
                busy || current || !ready || !production,
                does(Vercel::rollback, cx),
            ),
            (
                "Promote",
                Some(Icon::ArrowUp),
                busy || !ready || production,
                does(Vercel::promote, cx),
            ),
            match underway(&deployment.state) {
                true => ("Cancel Deployment", None, busy, does(Vercel::cancel, cx)),
                false => ("Redeploy", None, busy, does(Vercel::redeploy, cx)),
            },
            (
                "Inspect Deployment",
                None,
                inspector.is_none(),
                opens(inspector.clone()),
            ),
            (
                "View Source",
                None,
                inspector.is_none(),
                opens(inspector.map(|inspector| format!("{inspector}/source"))),
            ),
            ("Copy URL", None, url.is_none(), copies),
            (
                "Visit",
                Some(Icon::Globe),
                url.is_none() || !ready,
                opens(url),
            ),
        ];
        ui::menu_button(
            id.clone(),
            Anchor::TopRight,
            ui::IconButton::new(SharedString::from(format!("{id}-more")), Icon::More, label)
                .glyph(13.),
            move |mut popup, _, _| {
                for (index, (label, icon, disabled, act)) in items.iter().enumerate() {
                    if index == 2 {
                        popup = popup.separator();
                    }
                    let mut item = PopupMenuItem::new(*label).disabled(*disabled);
                    if let Some(icon) = icon {
                        item = item.icon(*icon);
                    }
                    if let Some(act) = act.clone() {
                        item = item.on_click(move |event, window, cx| act(event, window, cx));
                    }
                    popup = popup.item(item);
                }
                popup
            },
        )
        .into_any_element()
    }

    fn sign_in(
        &mut self,
        host: &Host,
        status: &Status,
        window: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let token = match &self.token {
            Some(token) => token.clone(),
            None => {
                let token = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder("Paste a Vercel token")
                        .masked(true)
                });
                self.typing
                    .push(cx.subscribe(&token, |pane, _, event: &InputEvent, cx| {
                        if let (InputEvent::PressEnter { .. }, Some((vercel, host))) =
                            (event, pane.parts::<Vercel>())
                        {
                            vercel.sign_in_with_token(host, cx);
                        }
                    }));
                self.token = Some(token.clone());
                token
            }
        };
        let failed = status
            .message
            .clone()
            .filter(|_| status.details.account.is_some());
        let ink = Tokens::get(cx).text_primary;
        ui::sign_in::Screen::new("vercel-sign-in", ui::picture(Icon::Vercel, 26., ink), "Connect Vercel")
            .lede("Your projects and their deployments beside the code, to roll back, promote or hand a failed build to an agent.")
            .foot(ui::sign_in::KEPT)
            .when(status.details.browser_sign_in, |screen| {
                screen
                    .child(super::browser_sign_in(
                        host,
                        "vercel-browser",
                        Icon::LogIn,
                        "Continue with Vercel",
                        |host, cx| host.sign_in_with_browser(cx),
                        cx,
                    ))
                    .child(ui::sign_in::or("or an access token", cx))
            })
            .child(ui::sign_in::Field::new("Access token", &token).mono())
            .child(
                ui::Button::new(
                    "vercel-token-sign-in",
                    if self.busy { "Signing in…" } else { "Sign in" },
                    if status.details.browser_sign_in {
                        ui::Variant::Outline
                    } else {
                        ui::Variant::Primary
                    },
                )
                .wide()
                .disabled(self.busy)
                .on_click(listen::<Vercel, ClickEvent>(cx, |vercel, host, _, _, cx| {
                    vercel.sign_in_with_token(host, cx)
                })),
            )
            .children(failed.map(|message| ui::sign_in::error(message, cx)))
            .into_any_element()
    }

    fn sign_in_with_token(&mut self, host: &mut Host, cx: &mut Context<PluginPane>) {
        let token = self
            .token
            .as_ref()
            .map(|token| token.read(cx).value().trim().to_owned())
            .unwrap_or_default();
        if token.is_empty() || self.busy {
            return;
        }
        self.busy = true;
        let signed = host.call("signIn", json!({ "token": token }));
        after(
            signed,
            |vercel: &mut Vercel, host, signed, cx| {
                vercel.busy = false;
                match signed {
                    Ok(_) => {
                        if let (Some(token), Some(window)) =
                            (vercel.token.clone(), cx.active_window())
                        {
                            let _ = window.update(cx, |_, window, cx| {
                                token.update(cx, |token, cx| token.set_value("", window, cx))
                            });
                        }
                    }
                    Err(failure) => host.said(&failure.titled("Sign in to Vercel"), cx),
                }
            },
            cx,
        );
    }

    fn sidebar(
        &self,
        status: &Status,
        team: &str,
        listed: Option<&Projects>,
        project: Option<&Project>,
        find: &Entity<InputState>,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let account = status.details.account.as_ref();
        let mut scopes: Vec<(String, String)> = Vec::new();
        if status.details.personal {
            let name = account.map_or_else(
                || "Personal".to_owned(),
                |account| match account.name.is_empty() {
                    true => account.username.clone(),
                    false => account.name.clone(),
                },
            );
            scopes.push((String::new(), name));
        }
        scopes.extend(
            status
                .details
                .teams
                .iter()
                .map(|each| (each.id.clone(), each.name.clone())),
        );
        let scope_rows = (scopes.len() > 1).then(|| {
            ui::Group::new("Team", "").children(scopes.into_iter().map(|(id, name)| {
                let chosen = id == team;
                ui::Row::new(SharedString::from(format!("vercel-team-{id}")), name)
                    .quiet()
                    .chosen(chosen)
                    .on_click(listen::<Vercel, ClickEvent>(
                        cx,
                        move |vercel, host, _, _, cx| {
                            if chosen {
                                return;
                            }
                            vercel.open(None, Section::Overview, cx);
                            let set = host.call("setTeam", json!({ "team": id }));
                            after(
                                set,
                                |_: &mut Vercel, host, set, cx| {
                                    if let Err(failure) = set {
                                        host.said(&failure.titled("Switch team"), cx);
                                    }
                                },
                                cx,
                            );
                        },
                    ))
            }))
        });
        let nav = match project {
            Some(project) => {
                let rows = SECTIONS.iter().map(|(section, label)| {
                    let (section, name) = (*section, project.name.clone());
                    ui::Row::new(
                        SharedString::from(format!("vercel-section-{label}")),
                        *label,
                    )
                    .quiet()
                    .chosen(self.section == section && self.deployment.is_none())
                    .on_click(listen::<Vercel, ClickEvent>(
                        cx,
                        move |vercel, _, _, _, cx| vercel.open(Some(name.clone()), section, cx),
                    ))
                });
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_0)
                    .children(rows)
                    .child(
                        ui::Row::new("vercel-all-projects", "All projects")
                            .quiet()
                            .on_click(listen::<Vercel, ClickEvent>(cx, |vercel, _, _, _, cx| {
                                vercel.open(None, Section::Overview, cx)
                            })),
                    )
                    .into_any_element()
            }
            None => ui::Group::new("Projects", "")
                .children(
                    listed
                        .into_iter()
                        .flat_map(|listed| &listed.projects)
                        .map(|each| {
                            let name = each.name.clone();
                            let state = each
                                .production
                                .as_ref()
                                .map_or("", |production| production.state.as_str());
                            ui::Row::new(
                                SharedString::from(format!("vercel-project-{}", each.id)),
                                each.name.clone(),
                            )
                            .quiet()
                            .mark(16., dot(state, &t))
                            .on_click(listen::<Vercel, ClickEvent>(
                                cx,
                                move |vercel, _, _, _, cx| {
                                    vercel.open(Some(name.clone()), Section::Overview, cx)
                                },
                            ))
                        }),
                )
                .into_any_element(),
        };
        let who = account.map_or_else(String::new, |account| match account.name.is_empty() {
            true => account.username.clone(),
            false => account.name.clone(),
        });
        div()
            .w(px(220.))
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(t.border)
            .flex()
            .flex_col()
            .gap(SPACE_2)
            .p(SPACE_2)
            .children(scope_rows)
            .child(ui::Field::new(find).icon(Icon::Search))
            .child(
                div()
                    .id("vercel-nav")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(nav),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_1H)
                    .px(SPACE_1)
                    .child(div().flex_1().min_w_0().child(ui::meta(who, cx)))
                    .child(
                        ui::IconButton::new("vercel-sign-out", Icon::LogOut, "Sign out of Vercel")
                            .on_click(listen::<Vercel, ClickEvent>(cx, |_, host, _, _, cx| {
                                let out = host.call("signOut", Value::Null);
                                after(
                                    out,
                                    |_: &mut Vercel, host, out, cx| {
                                        if let Err(failure) = out {
                                            host.said(&failure.titled("Sign out of Vercel"), cx);
                                        }
                                    },
                                    cx,
                                );
                            })),
                    ),
            )
            .into_any_element()
    }

    fn projects(&self, listed: &Projects, words: &str, cx: &mut Context<PluginPane>) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let shown: Vec<&Project> = listed
            .projects
            .iter()
            .filter(|each| words.is_empty() || each.name.to_lowercase().contains(words))
            .collect();
        if shown.is_empty() {
            return super::empty(
                match listed.projects.is_empty() {
                    true => "No projects in this team yet.",
                    false => "No project matches.",
                },
                cx,
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(SPACE_2)
            .children(shown.into_iter().map(|project| {
                let name = project.name.clone();
                let production = project.production.as_ref();
                let state = production.map_or("", |production| production.state.as_str());
                div()
                    .id(SharedString::from(format!("vercel-card-{}", project.id)))
                    .p(SPACE_3)
                    .rounded(RADIUS_2)
                    .border_1()
                    .border_color(t.border)
                    .cursor_pointer()
                    .hover(move |card| card.bg(t.surface_hover))
                    .flex()
                    .flex_col()
                    .gap(SPACE_1)
                    .on_click(listen::<Vercel, ClickEvent>(
                        cx,
                        move |vercel, _, _, _, cx| {
                            vercel.open(Some(name.clone()), Section::Overview, cx)
                        },
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(SPACE_2)
                            .child(dot(state, &t))
                            .child(
                                ui::text(project.name.clone(), 13., FontWeight::SEMIBOLD)
                                    .text_color(t.text_primary),
                            )
                            .children(
                                project
                                    .framework
                                    .clone()
                                    .map(|framework| ui::meta(framework, cx)),
                            ),
                    )
                    .children(
                        production
                            .and_then(|production| production.url.clone())
                            .map(|url| ui::mono(host_of(&url).to_owned(), 11.5, cx)),
                    )
                    .child(ui::meta(
                        [
                            project.repo.clone(),
                            Some(format!(
                                "updated {}",
                                crate::ui::time::ago(project.updated_at)
                            )),
                        ]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" · "),
                        cx,
                    ))
                    .into_any_element()
            }))
            .into_any_element()
    }

    fn overview(
        &mut self,
        host: &mut Host,
        team: &str,
        project: &Project,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let deployments = host
            .view(
                "deploymentsView",
                json!({ "team": team, "project": project.name }),
                cx,
            )
            .read::<Deployments>();
        let domains = host
            .view(
                "domainsView",
                json!({ "team": team, "project": project.name }),
                cx,
            )
            .read::<Domains>();
        let listed = match &deployments {
            Ok(Some(listed)) => listed.deployments.clone(),
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(failure, cx),
        };
        let production = project.production.as_ref();
        let current =
            production.and_then(|production| listed.iter().find(|each| each.id == production.id));
        let previous = listed.iter().find(|each| {
            each.target == Target::Production
                && each.state == "READY"
                && Some(&each.id) != production.map(|production| &production.id)
        });
        let domain_names: Vec<String> = domains
            .ok()
            .flatten()
            .map(|domains| {
                domains
                    .domains
                    .iter()
                    .filter(|domain| domain.redirect.is_none() && domain.git_branch.is_none())
                    .map(|domain| domain.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        let card = match (production, current) {
            (None, _) => super::empty("Nothing serves production yet.", cx),
            (Some(production), current) => {
                let mut actions: Vec<AnyElement> = Vec::new();
                if let (Some(previous), Some(_)) = (previous, current) {
                    let (team, previous) = (team.to_owned(), previous.clone());
                    actions.push(
                        ui::Button::new(
                            "vercel-rollback",
                            "Instant Rollback",
                            ui::Variant::Outline,
                        )
                        .disabled(self.busy)
                        .on_click(listen::<Vercel, ClickEvent>(
                            cx,
                            move |vercel, _, _, _, cx| vercel.rollback(&team, &previous, cx),
                        ))
                        .into_any_element(),
                    );
                }
                if let Some(url) = production.url.clone() {
                    actions.push(
                        ui::Button::new("vercel-visit", "Visit", ui::Variant::Outline)
                            .icon(Icon::Globe)
                            .on_click(move |_, _, cx| cx.open_url(&url))
                            .into_any_element(),
                    );
                }
                if let Some(current) = current {
                    actions.push(self.more(
                        "vercel-production-actions".into(),
                        "More actions".into(),
                        team,
                        current,
                        true,
                        cx,
                    ));
                }
                let opens = production.id.clone();
                box_of(
                    "Production Deployment",
                    actions,
                    div()
                        .id("vercel-production")
                        .flex()
                        .flex_col()
                        .gap(SPACE_1H)
                        .cursor_pointer()
                        .on_click(listen::<Vercel, ClickEvent>(
                            cx,
                            move |vercel, _, _, _, cx| {
                                vercel.deployment = Some(opens.clone());
                                cx.notify();
                            },
                        ))
                        .child(summary_line(production, current, cx))
                        .when(!domain_names.is_empty(), |card| {
                            card.child(ui::meta(
                                format!("Domains: {}", domain_names.join(", ")),
                                cx,
                            ))
                        })
                        .children(project.latest_production.as_ref().map(|latest| {
                            let opens = latest.id.clone();
                            ui::Button::new(
                                "vercel-latest",
                                format!(
                                    "The latest production build is {}",
                                    state_word(&latest.state).to_lowercase()
                                ),
                                ui::Variant::Outline,
                            )
                            .on_click(listen::<Vercel, ClickEvent>(
                                cx,
                                move |vercel, _, _, _, cx| {
                                    vercel.deployment = Some(opens.clone());
                                    cx.notify();
                                },
                            ))
                        }))
                        .into_any_element(),
                    &t,
                )
            }
        };
        let mut branches: Vec<&Deployment> = Vec::new();
        for each in listed.iter().filter(|each| each.target == Target::Preview) {
            if !branches.iter().any(|seen| seen.branch == each.branch) {
                branches.push(each);
            }
        }
        let branch_rows: Vec<AnyElement> = branches
            .into_iter()
            .map(|deployment| self.deployment_row(deployment, None, None, cx))
            .collect();
        div()
            .flex()
            .flex_col()
            .gap(SPACE_3H)
            .child(card)
            .child(box_of(
                "Active Branches",
                Vec::new(),
                match branch_rows.is_empty() {
                    true => ui::meta("No preview deployments.", cx).into_any_element(),
                    false => div()
                        .flex()
                        .flex_col()
                        .gap(SPACE_0)
                        .children(branch_rows)
                        .into_any_element(),
                },
                &t,
            ))
            .into_any_element()
    }

    fn deployments(
        &mut self,
        host: &mut Host,
        team: &str,
        project: &Project,
        words: &str,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let listed = match host
            .view(
                "deploymentsView",
                json!({ "team": team, "project": project.name }),
                cx,
            )
            .read::<Deployments>()
        {
            Ok(Some(listed)) => listed,
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(&failure, cx),
        };
        let environment = self.environment;
        let status = self.status;
        let shown: Vec<&Deployment> = listed
            .deployments
            .iter()
            .filter(|each| match environment {
                1 => each.target == Target::Production,
                2 => each.target == Target::Preview,
                _ => true,
            })
            .filter(|each| match status {
                1 => each.state == "READY",
                2 => each.state == "ERROR",
                3 => underway(&each.state),
                4 => each.state == "CANCELED",
                _ => true,
            })
            .filter(|each| {
                words.is_empty()
                    || [
                        &each.message,
                        &each.branch,
                        &each.commit,
                        &each.author,
                        &each.url,
                    ]
                    .into_iter()
                    .flatten()
                    .any(|field| field.to_lowercase().contains(words))
            })
            .collect();
        let production = project
            .production
            .as_ref()
            .map(|production| production.id.clone());
        let filters = div()
            .flex()
            .gap(SPACE_2)
            .child(
                ui::Segmented::words(
                    "vercel-environment",
                    ENVIRONMENTS
                        .iter()
                        .map(|word| ((*word).into(), None))
                        .collect(),
                    environment,
                )
                .on_select(listen::<Vercel, usize>(
                    cx,
                    |vercel, _, index, _, cx| {
                        vercel.environment = *index;
                        cx.notify();
                    },
                )),
            )
            .child(
                ui::Segmented::words(
                    "vercel-status",
                    STATUSES.iter().map(|word| ((*word).into(), None)).collect(),
                    status,
                )
                .on_select(listen::<Vercel, usize>(
                    cx,
                    |vercel, _, index, _, cx| {
                        vercel.status = *index;
                        cx.notify();
                    },
                )),
            );
        let rows: Vec<AnyElement> = shown
            .into_iter()
            .map(|deployment| {
                let current = Some(deployment.id.as_str()) == production.as_deref();
                let more = self.more(
                    format!("vercel-actions-{}", deployment.id).into(),
                    format!(
                        "Actions for {}",
                        deployment
                            .message
                            .as_deref()
                            .filter(|message| !message.is_empty())
                            .unwrap_or(&deployment.id)
                    )
                    .into(),
                    team,
                    deployment,
                    current,
                    cx,
                );
                self.deployment_row(deployment, production.as_deref(), Some(more), cx)
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap(SPACE_2H)
            .child(filters)
            .child(match rows.is_empty() {
                true => super::empty(
                    match listed.deployments.is_empty() {
                        true => "No deployments yet.",
                        false => "No deployment matches these filters.",
                    },
                    cx,
                ),
                false => div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_0)
                    .children(rows)
                    .into_any_element(),
            })
            .into_any_element()
    }

    fn deployment_row(
        &self,
        deployment: &Deployment,
        production: Option<&str>,
        more: Option<AnyElement>,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let opens = deployment.id.clone();
        let target = match (
            deployment.target,
            Some(deployment.id.as_str()) == production,
        ) {
            (Target::Production, true) => "Production · Current",
            (Target::Production, false) => "Production",
            (Target::Preview, _) => "Preview",
        };
        div()
            .id(SharedString::from(format!(
                "vercel-deployment-{}",
                deployment.id
            )))
            .px(SPACE_2H)
            .py(SPACE_2)
            .rounded(RADIUS_2)
            .cursor_pointer()
            .hover(move |row| row.bg(t.surface_hover))
            .flex()
            .items_center()
            .gap(SPACE_2H)
            .on_click(listen::<Vercel, ClickEvent>(
                cx,
                move |vercel, _, _, _, cx| {
                    vercel.deployment = Some(opens.clone());
                    cx.notify();
                },
            ))
            .child(dot(&deployment.state, &t))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(SPACE_0)
                    .child(
                        ui::text(name_of(deployment), 12.5, FontWeight::MEDIUM)
                            .text_color(t.text_primary)
                            .truncate(),
                    )
                    .child(ui::meta(
                        [
                            deployment.branch.clone(),
                            deployment
                                .commit
                                .as_ref()
                                .map(|commit| commit.chars().take(7).collect()),
                            deployment.author.clone(),
                        ]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" · "),
                        cx,
                    )),
            )
            .child(ui::meta(target, cx))
            .child(ui::meta(
                format!(
                    "{}{}",
                    state_word(&deployment.state),
                    deployment
                        .build_ms
                        .map_or_else(String::new, |ms| format!(" · {}", took(ms)))
                ),
                cx,
            ))
            .child(ui::meta(crate::ui::time::ago(deployment.created_at), cx))
            .children(more)
            .into_any_element()
    }

    fn deployment(
        &mut self,
        host: &mut Host,
        team: &str,
        project: &Project,
        id: &str,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let open = match host
            .view("logView", json!({ "team": team, "id": id }), cx)
            .read::<OpenLog>()
        {
            Ok(Some(open)) => open,
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(&failure, cx),
        };
        let deployment = &open.log.deployment;
        let current = project
            .production
            .as_ref()
            .map(|production| production.id.as_str())
            == Some(deployment.id.as_str());
        let ready = deployment.state == "READY";
        let mut actions: Vec<AnyElement> =
            vec![
                ui::Button::new("vercel-back", "Deployments", ui::Variant::Outline)
                    .on_click(listen::<Vercel, ClickEvent>(cx, |vercel, _, _, _, cx| {
                        vercel.deployment = None;
                        vercel.section = Section::Deployments;
                        cx.notify();
                    }))
                    .into_any_element(),
            ];
        let action = |id: &'static str,
                      label: &'static str,
                      act: fn(&mut Vercel, &str, &Deployment, &mut Context<PluginPane>),
                      cx: &mut Context<PluginPane>| {
            let (team, deployment) = (team.to_owned(), deployment.clone());
            ui::Button::new(id, label, ui::Variant::Outline)
                .disabled(self.busy)
                .on_click(listen::<Vercel, ClickEvent>(
                    cx,
                    move |vercel, _, _, _, cx| act(vercel, &team, &deployment, cx),
                ))
                .into_any_element()
        };
        if deployment.target == Target::Production && ready && !current {
            actions.push(action(
                "vercel-detail-rollback",
                "Instant Rollback",
                Vercel::rollback,
                cx,
            ));
        }
        if deployment.target == Target::Preview && ready {
            actions.push(action(
                "vercel-detail-promote",
                "Promote",
                Vercel::promote,
                cx,
            ));
        }
        actions.push(match underway(&deployment.state) {
            true => action(
                "vercel-detail-cancel",
                "Cancel Deployment",
                Vercel::cancel,
                cx,
            ),
            false => action("vercel-detail-redeploy", "Redeploy", Vercel::redeploy, cx),
        });
        actions.push(super::send_to_agent(
            "vercel-send",
            host,
            open.for_agent.clone(),
            cx,
        ));
        if let Some(url) = deployment.url.clone().filter(|_| ready) {
            actions.push(
                ui::Button::new("vercel-detail-visit", "Visit", ui::Variant::Outline)
                    .icon(Icon::Globe)
                    .on_click(move |_, _, cx| cx.open_url(&url))
                    .into_any_element(),
            );
        }
        actions.push(self.more(
            "vercel-detail-actions".into(),
            "More actions".into(),
            team,
            deployment,
            current,
            cx,
        ));
        let facts = [
            (
                "Status",
                Some(format!(
                    "{}{}",
                    state_word(&deployment.state),
                    deployment
                        .build_ms
                        .map_or_else(String::new, |ms| format!(" in {}", took(ms)))
                )),
            ),
            (
                "Environment",
                Some(match (deployment.target, current) {
                    (Target::Production, true) => "Production · Current".to_owned(),
                    (Target::Production, false) => "Production".to_owned(),
                    (Target::Preview, _) => "Preview".to_owned(),
                }),
            ),
            (
                "Created",
                Some(format!(
                    "{}{}",
                    crate::ui::time::ago(deployment.created_at),
                    deployment
                        .author
                        .as_ref()
                        .map_or_else(String::new, |author| format!(" by {author}"))
                )),
            ),
            (
                "Source",
                deployment.branch.as_ref().map(|branch| {
                    let commit = deployment
                        .commit
                        .as_ref()
                        .map_or_else(String::new, |commit| {
                            format!(" {}", commit.chars().take(7).collect::<String>())
                        });
                    let message = deployment
                        .message
                        .as_ref()
                        .map_or_else(String::new, |message| format!(" — {message}"));
                    format!("{branch}{commit}{message}")
                }),
            ),
            (
                "Domains",
                deployment
                    .aliases
                    .as_ref()
                    .filter(|aliases| !aliases.is_empty())
                    .map(|aliases| aliases.join(", ")),
            ),
        ];
        let lines = &open.log.lines;
        let counted = match open.log.skipped {
            0 => format!("{} lines", lines.len()),
            skipped => format!("last {} of {} lines", lines.len(), lines.len() + skipped),
        };
        let log = div()
            .rounded(RADIUS_2)
            .border_1()
            .border_color(t.border)
            .bg(t.surface_sunken)
            .p(SPACE_2H)
            .flex()
            .flex_col()
            .font_family(theme::MONO)
            .text_size(crate::theme::ui_text(TEXT_2XS_H))
            .children(match lines.is_empty() {
                true => vec![ui::meta("Nothing printed yet.", cx).into_any_element()],
                false => lines
                    .iter()
                    .map(|line| {
                        div()
                            .text_color(match looks_like_error(line) {
                                true => t.danger,
                                false => t.text_secondary,
                            })
                            .child(match line.text.is_empty() {
                                true => " ".to_owned(),
                                false => line.text.clone(),
                            })
                            .into_any_element()
                    })
                    .collect(),
            });
        div()
            .flex()
            .flex_col()
            .gap(SPACE_3H)
            .child(div().flex().flex_wrap().gap(SPACE_1H).children(actions))
            .children(deployment.error.clone().map(|error| {
                div()
                    .p(SPACE_2H)
                    .rounded(RADIUS_2)
                    .border_1()
                    .border_color(t.danger)
                    .child(ui::text(error, 12., FontWeight::MEDIUM).text_color(t.danger))
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_1H)
                    .children(facts.into_iter().filter_map(|(label, value)| {
                        let value = value?;
                        Some(
                            div()
                                .flex()
                                .gap(SPACE_3)
                                .child(div().w(px(110.)).flex_none().child(ui::meta(label, cx)))
                                .child(
                                    ui::text(value, 12.5, FontWeight::NORMAL)
                                        .text_color(t.text_primary),
                                ),
                        )
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .child(
                        ui::text("Build Logs", 13., FontWeight::SEMIBOLD)
                            .text_color(t.text_primary),
                    )
                    .child(ui::meta(counted, cx)),
            )
            .child(log)
            .into_any_element()
    }

    fn environment(
        &mut self,
        host: &mut Host,
        team: &str,
        project: &Project,
        words: &str,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let variables = match host
            .view(
                "envView",
                json!({ "team": team, "project": project.name }),
                cx,
            )
            .read::<Variables>()
        {
            Ok(Some(variables)) => variables,
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(&failure, cx),
        };
        let shown: Vec<_> = variables
            .variables
            .iter()
            .filter(|variable| words.is_empty() || variable.key.to_lowercase().contains(words))
            .collect();
        let body = match shown.is_empty() {
            true => super::empty(
                match variables.variables.is_empty() {
                    true => "No Environment Variables Added",
                    false => "No variable matches.",
                },
                cx,
            ),
            false => div()
                .flex()
                .flex_col()
                .gap(SPACE_0)
                .children(shown.into_iter().map(|variable| {
                    let targets = match variable.target.len() {
                        3 => "All Environments".to_owned(),
                        _ => variable
                            .target
                            .iter()
                            .map(|target| state_word(target))
                            .collect::<Vec<_>>()
                            .join(", "),
                    };
                    div()
                        .px(SPACE_2H)
                        .py(SPACE_2)
                        .flex()
                        .items_center()
                        .gap(SPACE_3)
                        .border_b_1()
                        .border_color(t.border)
                        .child(div().flex_1().min_w_0().child(ui::mono(
                            variable.key.clone(),
                            12.,
                            cx,
                        )))
                        .child(ui::meta(targets, cx))
                        .children(
                            variable
                                .git_branch
                                .clone()
                                .map(|branch| ui::meta(branch, cx)),
                        )
                        .children(
                            variable
                                .updated_at
                                .map(|at| ui::meta(crate::ui::time::ago(at), cx)),
                        )
                }))
                .into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .gap(SPACE_2H)
            .child(ui::meta(
                "Values stay in Vercel; only the names are shown here.",
                cx,
            ))
            .child(body)
            .into_any_element()
    }

    fn domains(
        &mut self,
        host: &mut Host,
        team: &str,
        project: &Project,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let domains = match host
            .view(
                "domainsView",
                json!({ "team": team, "project": project.name }),
                cx,
            )
            .read::<Domains>()
        {
            Ok(Some(domains)) => domains,
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(&failure, cx),
        };
        if domains.domains.is_empty() {
            return super::empty("No domains.", cx);
        }
        div()
            .flex()
            .flex_col()
            .gap(SPACE_2)
            .children(domains.domains.iter().map(|domain| {
                let url = format!("https://{}", domain.name);
                let role = match (&domain.redirect, &domain.git_branch) {
                    (Some(redirect), _) => format!("Redirects to {redirect}"),
                    (None, Some(branch)) => format!("Assigned to the {branch} branch"),
                    (None, None) => "Production".to_owned(),
                };
                let (word, colour) = match domain.verified {
                    true => ("Valid Configuration", t.live),
                    false => ("Invalid Configuration", t.danger),
                };
                div()
                    .p(SPACE_3)
                    .rounded(RADIUS_2)
                    .border_1()
                    .border_color(t.border)
                    .flex()
                    .items_center()
                    .gap(SPACE_3)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(SPACE_0)
                            .child(
                                ui::text(domain.name.clone(), 13., FontWeight::SEMIBOLD)
                                    .text_color(t.text_primary),
                            )
                            .child(ui::meta(role, cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(SPACE_1H)
                            .child(div().size(px(8.)).rounded_full().bg(colour))
                            .child(ui::meta(word, cx)),
                    )
                    .child(
                        ui::IconButton::new(
                            SharedString::from(format!("vercel-domain-{}", domain.name)),
                            Icon::Globe,
                            "Visit",
                        )
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                    )
            }))
            .into_any_element()
    }
}

/// What a deployment is called in lists: its commit's message, else its address, else its id.
fn name_of(deployment: &Deployment) -> String {
    deployment
        .message
        .clone()
        .or_else(|| deployment.url.as_deref().map(|url| host_of(url).to_owned()))
        .unwrap_or_else(|| deployment.id.clone())
}

/// The deployment serving production, in one line: where, how it stands and when.
fn summary_line(
    production: &Production,
    current: Option<&Deployment>,
    cx: &mut Context<PluginPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let address = production
        .url
        .as_deref()
        .map(host_of)
        .unwrap_or(&production.id)
        .to_owned();
    div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(dot(&production.state, &t))
        .child(ui::text(address, 13., FontWeight::SEMIBOLD).text_color(t.text_primary))
        .child(ui::meta(
            [
                Some(state_word(&production.state)),
                current.and_then(|current| current.branch.clone()),
                current.map(name_of),
                Some(crate::ui::time::ago(production.created_at)),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · "),
            cx,
        ))
        .into_any_element()
}

fn box_of(
    title: &'static str,
    actions: Vec<AnyElement>,
    body: AnyElement,
    t: &Tokens,
) -> AnyElement {
    div()
        .rounded(RADIUS_2)
        .border_1()
        .border_color(t.border)
        .flex()
        .flex_col()
        .child(
            div()
                .px(SPACE_3)
                .py(SPACE_2)
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .border_b_1()
                .border_color(t.border)
                .child(ui::text(title, 12.5, FontWeight::SEMIBOLD).text_color(t.text_primary))
                .child(div().flex_1())
                .children(actions),
        )
        .child(div().p(SPACE_3).child(body))
        .into_any_element()
}
