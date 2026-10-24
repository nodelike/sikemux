//! The ports the project's terminals, tasks and agents listen on, as the top bar lists them.

use std::path::Path;
use std::rc::Rc;

use gpui::Anchor;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::component::WindowExt;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, ClipboardItem, Context, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, WeakEntity,
};
use serde_json::Value;
use sikemux_wire::protocol::PaneKind;

use crate::icons::Icon;
use crate::theme::Tokens;
use crate::toast;
use crate::ui;
use crate::workbench::Workbench;

/// One listening port, with who listens on it as the person reads it.
pub struct PortRow {
    pub port: u64,
    process: String,
    url: String,
    preview: bool,
    owner: Owner,
    /// A task's, agent's or terminal's name.
    label: String,
    /// The tab that shows who listens, by its kind and target.
    reveal: Option<(&'static str, String)>,
    /// The task run listening, which stopping it ends.
    pub execution: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Owner {
    Terminal,
    Task,
    Agent,
}

impl PortRow {
    pub fn new(port: &Value, bench: &Workbench) -> Option<Self> {
        let owner = &port["owner"];
        let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
        let session = owner["sessionId"].as_u64().map(|id| id.to_string());
        let agent = owner["agentId"].as_str().and_then(|id| {
            bench
                .view
                .as_ref()?
                .workspace
                .agents
                .iter()
                .find(|agent| agent.id == id)
        });
        let (kind, label, reveal) = match (owner["kind"].as_str()?, agent) {
            ("agent", agent) => (
                Owner::Agent,
                text(&owner["label"]),
                agent.map(|agent| ("agent", agent.id.clone())),
            ),
            ("task", Some(agent)) => (
                Owner::Agent,
                agent.title.clone(),
                Some(("agent", agent.id.clone())),
            ),
            ("task", None) => (
                Owner::Task,
                text(&owner["label"]),
                session.map(|session| ("terminal", session)),
            ),
            _ => {
                let tab = session.as_ref().and_then(|session| {
                    let strip = bench.strip()?;
                    bench.tabs(&strip).iter().find(|tab| {
                        crate::tabs::first_view(&tab.pane) == Some((PaneKind::Terminal, session))
                    })
                });
                (
                    Owner::Terminal,
                    tab.map_or_else(
                        || "terminal".to_owned(),
                        |tab| crate::tabs::label(bench, tab).to_string(),
                    ),
                    tab.and(session).map(|session| ("terminal", session)),
                )
            }
        };
        Some(Self {
            port: port["port"].as_u64()?,
            process: text(&port["process"]),
            url: text(&port["url"]),
            preview: port["preview"] == true,
            owner: kind,
            label,
            reveal,
            execution: owner["executionId"].as_str().map(str::to_owned),
        })
    }

    /// The row the Run menu's "Running now" shows: it reveals who listens, and stops a task.
    pub fn running_item(&self, bench: &WeakEntity<Workbench>) -> PopupMenuItem {
        let port = self.port;
        let process = if self.process.is_empty() {
            "process".to_owned()
        } else {
            self.process.clone()
        };
        let owner = if self.owner == Owner::Agent {
            self.label.clone()
        } else {
            "You".to_owned()
        };
        let stop = self
            .execution
            .clone()
            .map(|execution| (bench.clone(), execution));
        PopupMenuItem::element(move |_, cx| {
            let t = Tokens::get(cx).clone();
            let stop = stop.clone().map(|(bench, execution)| {
                div()
                    .id(("stop-port", port as usize))
                    .p(px(3.))
                    .rounded(crate::theme::RADIUS_2)
                    .hover(move |style| style.bg(t.surface_hover))
                    .tooltip(ui::tooltip(format!("Stop what listens on {port}")))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        let _ = bench.update(cx, |bench, _| bench.stop_run(&execution));
                    })
                    .child(ui::icon(Icon::Stop, 9.).text_color(t.danger))
            });
            div()
                .w_full()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(div().size(px(7.)).rounded_full().bg(t.live))
                .child(div().flex_1().min_w_0().truncate().child(process.clone()))
                .child(
                    div()
                        .font_family(crate::theme::MONO)
                        .text_color(t.text_tertiary)
                        .child(format!(":{port}")),
                )
                .child(div().text_color(t.text_tertiary).child(owner.clone()))
                .children(stop)
        })
        .on_click(self.reveal_click(bench))
    }

    /// The row the ports chip's menu shows: it opens the port in the browser.
    fn item(&self, bench: &WeakEntity<Workbench>, desk: Option<&str>) -> PopupMenuItem {
        let (port, preview, kind, label) =
            (self.port, self.preview, self.owner, self.label.clone());
        let runtime = runtime(&self.process);
        let process = self.process.clone();
        let (copied, opened, browsed) = (self.url.clone(), self.url.clone(), self.url.clone());
        let desk = desk.map(str::to_owned);
        let on_desk = desk.is_some();
        let opener = bench.clone();
        let reveal = self.reveal.is_some().then(|| self.reveal_click(bench));
        PopupMenuItem::element(move |_, cx| {
            let t = Tokens::get(cx).clone();
            let lead = runtime
                .and_then(|runtime| crate::file_icons::named(runtime, cx))
                .map(|icon| gpui_kit::img(icon).size(px(14.)).into_any_element())
                .unwrap_or_else(|| {
                    ui::icon(Icon::Globe, 13.)
                        .text_color(t.text_tertiary)
                        .into_any_element()
                });
            let copied = copied.clone();
            let action = |id: &'static str, glyph: Icon, about: SharedString| {
                div()
                    .id((id, port as usize))
                    .p(px(3.))
                    .rounded(crate::theme::RADIUS_2)
                    .hover(move |style| style.bg(t.surface_hover))
                    .tooltip(ui::tooltip(about))
                    .child(ui::icon(glyph, 12.).text_color(t.text_tertiary))
            };
            div()
                .w_full()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .flex_none()
                        .id(("port-process", port as usize))
                        .when(!process.is_empty(), |lead| {
                            lead.tooltip(ui::tooltip(process.clone()))
                        })
                        .child(lead),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .font_family(crate::theme::MONO)
                        .child(format!(":{port}"))
                        .when(preview, |address| {
                            address.child(div().size(px(5.)).rounded_full().bg(t.live))
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .text_color(t.text_tertiary)
                        .child(
                            ui::icon(
                                match kind {
                                    Owner::Agent => Icon::Agent,
                                    Owner::Task => Icon::Run,
                                    Owner::Terminal => Icon::Terminal,
                                },
                                11.,
                            )
                            .text_color(t.text_tertiary),
                        )
                        .child(div().min_w_0().truncate().child(label.clone())),
                )
                .when(on_desk, |row| {
                    let browsed = browsed.clone();
                    row.child(
                        action("browse-port", Icon::Globe, "Open in your browser".into()).on_click(
                            move |_, _, cx| {
                                cx.stop_propagation();
                                cx.open_url(&browsed);
                            },
                        ),
                    )
                })
                .child(action("copy-port", Icon::Copy, "Copy URL".into()).on_click(
                    move |_, window, cx| {
                        cx.stop_propagation();
                        cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
                        window.push_notification(toast::success(format!("Copied {copied}")), cx);
                    },
                ))
                .children(reveal.clone().map(|reveal| {
                    action(
                        "show-port-owner",
                        if kind == Owner::Agent {
                            Icon::Agent
                        } else {
                            Icon::Window
                        },
                        format!("Show {label}").into(),
                    )
                    .on_click(move |event, window, cx| {
                        cx.stop_propagation();
                        reveal(event, window, cx);
                    })
                }))
        })
        .on_click(move |_, _, cx| {
            let shown = desk.as_ref().is_some_and(|agent| {
                opener
                    .update(cx, |bench, _| bench.open_on_desk(agent, &opened))
                    .unwrap_or(false)
            });
            if !shown {
                cx.open_url(&opened);
            }
        })
    }

    fn reveal_click(
        &self,
        bench: &WeakEntity<Workbench>,
    ) -> impl Fn(&gpui_kit::ClickEvent, &mut gpui_kit::Window, &mut App) + Clone + 'static {
        let (bench, reveal) = (bench.clone(), self.reveal.clone());
        move |_, _, cx| {
            if let Some((kind, target)) = &reveal {
                let _ = bench.update(cx, |bench, _| bench.show(kind, target, None));
            }
        }
    }
}

/// The runtime behind a listening process, as a file icon names it, when its name says.
fn runtime(process: &str) -> Option<&'static str> {
    const RUNTIMES: &[(&[&str], &str)] = &[
        (
            &[
                "node",
                "next-server",
                "nuxt",
                "vite",
                "npm",
                "pnpm",
                "yarn",
                "bun",
                "tsx",
                "esbuild",
                "webpack",
                "astro",
                "remix",
            ],
            "nodejs",
        ),
        (&["deno"], "deno"),
        (
            &[
                "python",
                "uvicorn",
                "gunicorn",
                "hypercorn",
                "flask",
                "django",
            ],
            "python",
        ),
        (&["ruby", "puma", "rails", "unicorn"], "ruby"),
        (&["java", "gradle", "mvn"], "java"),
        (&["php", "php-fpm"], "php"),
        (&["beam.smp", "elixir", "mix"], "elixir"),
    ];
    let name = Path::new(process.trim())
        .file_name()?
        .to_string_lossy()
        .to_lowercase();
    if ["com.docker", "docker", "vpnkit", "orbstack"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
    {
        return Some("docker");
    }
    let word_ends = |rest: &str| {
        !rest
            .chars()
            .next()
            .is_some_and(|next| next.is_alphanumeric() || next == '_')
    };
    RUNTIMES.iter().find_map(|(names, icon)| {
        names
            .iter()
            .any(|known| {
                name.strip_prefix(known).is_some_and(|rest| {
                    let rest = if *known == "python" {
                        rest.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.')
                    } else {
                        rest
                    };
                    word_ends(rest)
                })
            })
            .then_some(*icon)
    })
}

/// How many ports the project's terminals, tasks and agents listen on, opening their list.
pub fn chip(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let t = Tokens::get(cx).clone();
    let ports = bench.read("ports")?.as_array()?;
    if ports.is_empty() {
        return None;
    }
    let count = ports.len();
    let rows: Vec<PortRow> = ports
        .iter()
        .filter_map(|port| PortRow::new(port, bench))
        .collect();
    let rows = Rc::new(rows);
    let desk = Rc::new(desk_agent(bench));
    let weak = cx.entity().downgrade();
    let about = if count == 1 {
        "1 listening port".to_owned()
    } else {
        format!("{count} listening ports")
    };
    Some(
        Button::new("ports-chip")
            .ghost()
            .compact()
            .h(px(24.))
            .px(px(4.))
            .gap(px(5.))
            .rounded(crate::theme::RADIUS_1)
            .font_family(crate::theme::MONO)
            .text_size(crate::theme::ui_text(11.))
            .text_color(t.text_tertiary)
            .tooltip(about)
            .child(ui::icon(Icon::Plug, 12.).text_color(t.text_tertiary))
            .child(div().text_color(t.text_primary).child(count.to_string()))
            .dropdown_menu_with_anchor(Anchor::TopRight, move |popup, _, _| {
                ports_menu(popup, &rows, &desk, &weak)
            })
            .into_any_element(),
    )
}

fn ports_menu(
    popup: PopupMenu,
    rows: &[PortRow],
    desk: &Option<(String, String)>,
    bench: &WeakEntity<Workbench>,
) -> PopupMenu {
    let mut popup = popup
        .scrollable(true)
        .min_w(px(280.))
        .max_w(px(420.))
        .max_h(px(480.))
        .label(match desk {
            Some((_, title)) => format!("Opens on {title}'s desk"),
            None => "No agent running · opens in your browser".to_owned(),
        });
    for row in rows {
        popup = popup.item(row.item(bench, desk.as_ref().map(|(id, _)| id.as_str())));
    }
    popup
}

/// The agent whose desk a port opens on: the one in front while it is awake, else the
/// awake one that worked last; none when every agent is asleep.
fn desk_agent(bench: &Workbench) -> Option<(String, String)> {
    let agents = &bench.view.as_ref()?.workspace.agents;
    let awake = |id: &str| agents.iter().any(|agent| agent.id == id && !agent.asleep);
    let front = bench.agent().filter(|front| awake(&front.id));
    let chosen = front.or_else(|| {
        let project = bench.project()?;
        bench
            .agents_of(&project.id)
            .into_iter()
            .filter(|agent| awake(&agent.id))
            .max_by_key(|agent| agent.active_at)
    })?;
    Some((chosen.id, chosen.title))
}
