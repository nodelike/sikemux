//! AWS's pane: the services of the profile it shows, each a list from the plugin's views with the
//! one picked beside it, ECS from its clusters down to a service's tasks, and a log group's live
//! tail.

use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use gpui_kit::component::button::{Button as KitButton, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::TestSupportExt as _;
use gpui_kit::{
    div, px, uniform_list, AnyElement, App, ClickEvent, Context, Div, FontWeight,
    InteractiveElement, IntoElement, ParentElement, ScrollStrategy, SharedString,
    StatefulInteractiveElement, Styled, Task, UniformListScrollHandle, Window,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use sikemux_plugin_aws_views::{
    Account, AgentProfiles, Billing, BillingChange, BillingMonth, Ec2, EcsClusters, EcsServices,
    EcsTasks, Health, Lambda, LogLevel, LogLine, Sqs, S3,
};

use super::{after, listen, Failure, Host, Panel, PluginPane, Streamed};
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H, SPACE_4,
    TEXT_2XS,
};
use crate::ui::time::ago_written as ago;
use crate::ui::{self, Button, IconButton, Pick, Segmented, Toggle, Tone, Variant};

pub const REGISTRATION: super::Registration = super::Registration {
    plugin: "sikemux.aws",
    surface: "console",
    pane: super::boxed::<Aws>,
    top_bar: None,
    palettes: &[],
};

const PITCH: &str = "Your clusters, functions, queues and costs, beside the code.";
/// How many closed months the costs reach back over, beside the month in progress.
const MONTHS_BACK: u32 = 5;
/// The most lines a tail keeps, the oldest going first.
const TAIL_LINES: usize = 5000;
const LINE: f32 = 20.;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Service {
    #[default]
    Ecs,
    Ec2,
    Lambda,
    Sqs,
    Billing,
    S3,
}

const SERVICES: [Service; 6] = [
    Service::Ecs,
    Service::Ec2,
    Service::Lambda,
    Service::Sqs,
    Service::Billing,
    Service::S3,
];

impl Service {
    fn label(self) -> &'static str {
        match self {
            Service::Ecs => "ECS",
            Service::Ec2 => "EC2",
            Service::Lambda => "Lambda",
            Service::Sqs => "SQS",
            Service::Billing => "Billing",
            Service::S3 => "S3",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Service::Ecs => Icon::Stack,
            Service::Ec2 => Icon::Server,
            Service::Lambda => Icon::Bolt,
            Service::Sqs => Icon::Inbox,
            Service::Billing => Icon::Activity,
            Service::S3 => Icon::Folder,
        }
    }
}

/// How far into ECS the pane is: its clusters, or one cluster's services.
#[derive(Clone, PartialEq, Default)]
enum Ecs {
    #[default]
    Clusters,
    Services(String),
}

#[derive(Default)]
pub struct Aws {
    service: Service,
    ecs: Ecs,
    /// The row picked in each list, by the list's name.
    picked: HashMap<&'static str, String>,
    tail: Option<Tail>,
}

/// A log group's live tail, read while it is open.
struct Tail {
    title: String,
    group: String,
    stream: Option<String>,
    lines: Rc<VecDeque<LogLine>>,
    live: bool,
    failed: Option<SharedString>,
    /// Which lines show: all, warnings and errors, or errors.
    level: usize,
    follow: bool,
    scroll: UniformListScrollHandle,
    _reading: Task<()>,
}

impl Tail {
    fn take(&mut self, streamed: Streamed) {
        match streamed {
            Streamed::Item(item) => {
                let line = match serde_json::from_value::<LogLine>(item) {
                    Ok(line) => line,
                    Err(error) => {
                        eprintln!("sikemux: a CloudWatch log line did not read: {error}");
                        return;
                    }
                };
                let kept = Rc::make_mut(&mut self.lines);
                kept.push_back(line);
                if kept.len() > TAIL_LINES {
                    kept.pop_front();
                }
                if self.follow {
                    let last = kept.len().saturating_sub(1);
                    self.scroll.scroll_to_item(last, ScrollStrategy::Nearest);
                }
            }
            Streamed::Ended(failure) => {
                self.live = false;
                self.failed = failure.map(|failure| failure.message);
            }
        }
    }

    fn shows(&self, line: &LogLine) -> bool {
        match self.level {
            1 => line.level.is_some(),
            2 => line.level == Some(LogLevel::Error),
            _ => true,
        }
    }
}

/// A row of a list: how it stands, what it is, and what it says beside that.
struct Item {
    key: String,
    tone: Option<Tone>,
    title: SharedString,
    sub: Option<SharedString>,
    aside: Option<SharedString>,
    /// How full its bar is, for lists that compare their rows.
    bar: Option<f64>,
    dim: bool,
}

impl Item {
    fn new(key: impl Into<String>, title: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            tone: None,
            title: title.into(),
            sub: None,
            aside: None,
            bar: None,
            dim: false,
        }
    }
}

/// A fact about the row picked, and whether it can be copied.
struct Fact {
    label: &'static str,
    value: Option<String>,
    copy: bool,
}

fn fact(label: &'static str, value: impl Into<Option<String>>) -> Fact {
    Fact {
        label,
        value: value.into(),
        copy: false,
    }
}

fn copyable(label: &'static str, value: impl Into<Option<String>>) -> Fact {
    Fact {
        copy: true,
        ..fact(label, value)
    }
}

impl Panel for Aws {
    fn render(
        &mut self,
        host: &mut Host,
        _: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let view = match host
            .view("accountView", Value::Null, cx)
            .read::<AccountView>()
        {
            Ok(None) => return super::loading(cx),
            Err(failure) => return super::failure(&failure, cx),
            Ok(Some(view)) => view,
        };
        let account = view.account();
        let Some(profile) = account.profile.clone() else {
            return choose_profile(&account, cx);
        };
        if host.signed_in(cx) == Some(false) {
            return sign_in(host, &profile, cx);
        }
        let main = match self.tail.is_some() {
            true => self.tail_page(cx),
            false => self.service_page(host, &profile, cx),
        };
        div()
            .size_full()
            .flex()
            .child(self.rail(host, &account, &view.kinds(), &profile, cx))
            .child(div().flex_1().min_w_0().h_full().flex().child(main))
            .into_any_element()
    }
}

impl Aws {
    fn rail(
        &self,
        host: &mut Host,
        account: &Account,
        kinds: &HashMap<String, String>,
        profile: &str,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let signed_in = host.signed_in(cx);
        let switch = account_switch(account, kinds, profile, signed_in, cx);
        let t = Tokens::get(cx).clone();
        let services = SERVICES.iter().enumerate().map(|(index, service)| {
            let (chosen, service) = (*service == self.service, *service);
            let hover = t.surface_hover;
            div()
                .id(("aws-service", index))
                .px(SPACE_2)
                .py(SPACE_1H)
                .rounded(theme::RADIUS_2)
                .flex()
                .items_center()
                .gap(SPACE_2)
                .cursor_pointer()
                .when(chosen, |row| row.bg(t.surface_chosen))
                .when(!chosen, |row| row.hover(move |row| row.bg(hover)))
                .on_click(listen::<Aws, ClickEvent>(cx, move |aws, _, _, _, cx| {
                    (aws.service, aws.tail) = (service, None);
                    cx.notify();
                }))
                .child(ui::icon(service.icon(), 14.).text_color(t.text_secondary))
                .child(
                    ui::text(service.label(), 12.5, FontWeight::MEDIUM).text_color(t.text_primary),
                )
        });
        div()
            .w(px(200.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(1.))
            .p(SPACE_1H)
            .border_r_1()
            .border_color(t.border)
            .child(
                div()
                    .px(SPACE_2)
                    .pt(SPACE_1)
                    .pb(SPACE_1)
                    .child(ui::caps("Services", cx)),
            )
            .children(services)
            .child(div().flex_1())
            .child(self.agents(host, cx))
            .child(div().pt(SPACE_1H).child(switch))
            .into_any_element()
    }

    /// The profile agents start on, in the project in front and in every other one.
    fn agents(&self, host: &mut Host, cx: &mut Context<PluginPane>) -> AnyElement {
        let project = host.project(cx);
        let read = host
            .view("agentProfileView", json!({ "project": project }), cx)
            .read::<AgentProfiles>();
        let Ok(Some(picks)) = read else {
            return div().into_any_element();
        };
        let mut body = div().flex().flex_col().gap(SPACE_1);
        match &picks.from_shell {
            Some(note) => body = body.child(div().px(SPACE_2).child(ui::meta(note.clone(), cx))),
            None => {
                if let Some(project) = &project {
                    let name = project.rsplit('/').next().unwrap_or(project).to_owned();
                    body = body
                        .child(div().px(SPACE_2).child(ui::meta(name, cx)))
                        .child(agent_pick(
                            "aws-agents-project",
                            &picks,
                            picks.project.clone(),
                            picks.project_unset.clone(),
                            Some(project.clone()),
                            cx,
                        ));
                }
                let every = if project.is_some() {
                    "Other projects"
                } else {
                    "Every project"
                };
                body = body
                    .child(div().px(SPACE_2).child(ui::meta(every, cx)))
                    .child(agent_pick(
                        "aws-agents-default",
                        &picks,
                        picks.default.clone(),
                        picks.default_unset.clone(),
                        None,
                        cx,
                    ));
            }
        }
        div()
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .child(div().px(SPACE_2).child(ui::caps("Agents use", cx)))
            .child(body)
            .into_any_element()
    }

    fn service_page(
        &mut self,
        host: &mut Host,
        profile: &str,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let page = match self.service {
            Service::Ecs => match self.ecs.clone() {
                Ecs::Clusters => self.clusters(host, profile, cx),
                Ecs::Services(cluster) => self.ecs_services(host, profile, &cluster, cx),
            },
            Service::Ec2 => self.ec2(host, profile, cx),
            Service::Lambda => self.lambda(host, profile, cx),
            Service::Sqs => self.sqs(host, profile, cx),
            Service::Billing => self.billing(host, profile, cx),
            Service::S3 => self.s3(host, profile, cx),
        };
        page.unwrap_or_else(|shown| shown)
    }

    /// The row picked in list `list`, else its first.
    fn chosen<'a, T>(
        &self,
        list: &'static str,
        rows: &'a [T],
        key: impl Fn(&T) -> &str,
    ) -> Option<&'a T> {
        let picked = self.picked.get(list);
        rows.iter()
            .find(|row| picked.is_some_and(|picked| key(row) == picked))
            .or_else(|| rows.first())
    }

    fn clusters(
        &mut self,
        host: &mut Host,
        profile: &str,
        cx: &mut Context<PluginPane>,
    ) -> Result<AnyElement, AnyElement> {
        let view: Rc<EcsClusters> = read(
            host,
            "ecsClustersView",
            json!({ "profile": profile }),
            profile,
            cx,
        )?;
        let chosen = self.chosen("ecs-clusters", &view.clusters, |c| &c.name);
        let items = view
            .clusters
            .iter()
            .map(|cluster| Item {
                tone: Some(tone(cluster.health)),
                sub: Some(cluster.env_label.clone().into()),
                aside: Some(
                    match cluster.pending {
                        0 => format!("{} tasks", cluster.running),
                        pending => format!("{} tasks +{pending}", cluster.running),
                    }
                    .into(),
                ),
                ..Item::new(cluster.name.clone(), cluster.short_name.clone())
            })
            .collect();
        let detail = chosen.map(|cluster| {
            let name = cluster.name.clone();
            let open = Button::new("aws-open-cluster", "Open cluster", Variant::Primary).on_click(
                listen::<Aws, ClickEvent>(cx, move |aws, _, _, _, cx| {
                    aws.ecs = Ecs::Services(name.clone());
                    cx.notify();
                }),
            );
            detail(
                cluster.product.clone(),
                Some(cluster.arn.clone()),
                vec![open.into_any_element(), console(&cluster.console_url)],
                vec![
                    fact("Environment", cluster.env_label.clone()),
                    fact("Services", cluster.services.to_string()),
                    fact("Running tasks", cluster.running.to_string()),
                    fact("Pending tasks", cluster.pending.to_string()),
                    copyable("ARN", cluster.arn.clone()),
                ],
                Vec::new(),
                cx,
            )
        });
        let list = self.list(
            "ecs-clusters",
            heading(
                vec!["ECS".into(), "Clusters".into()],
                view.clusters.len(),
                None,
                cx,
            ),
            items,
            chosen.map(|c| c.name.as_str()),
            "No clusters in this region.",
            cx,
        );
        Ok(two(list, detail, cx))
    }

    fn ecs_services(
        &mut self,
        host: &mut Host,
        profile: &str,
        cluster: &str,
        cx: &mut Context<PluginPane>,
    ) -> Result<AnyElement, AnyElement> {
        let params = json!({ "profile": profile, "cluster": cluster });
        let view: Rc<EcsServices> = read(host, "ecsServicesView", params, profile, cx)?;
        let chosen = self.chosen("ecs-services", &view.services, |s| &s.name);
        let items = view
            .services
            .iter()
            .map(|service| Item {
                tone: Some(tone(service.health)),
                sub: Some(
                    match &service.deployed_at {
                        Some(at) => format!("{} · deployed {}", service.state_label, ago(at)),
                        None => service.state_label.clone(),
                    }
                    .into(),
                ),
                aside: Some(format!("{}/{}", service.running, service.desired).into()),
                ..Item::new(service.name.clone(), service.name.clone())
            })
            .collect();
        let detail = match chosen {
            None => None,
            Some(service) => {
                let tasks = self.tasks(host, profile, cluster, &service.name, cx);
                let (profile, cluster_name, name) =
                    (profile.to_owned(), cluster.to_owned(), service.name.clone());
                let tail = Button::new("aws-service-tail", "Tail logs", Variant::Primary)
                    .icon(Icon::LogLines)
                    .on_click(listen::<Aws, ClickEvent>(cx, move |_, host, _, _, cx| {
                        let asked = host.call(
                            "ecsServiceLogConfig",
                            json!({ "profile": profile, "cluster": cluster_name, "service": name }),
                        );
                        let (profile, title) = (profile.clone(), name.clone());
                        after::<Aws, _>(
                            asked,
                            move |aws, host, answer, cx| match answer {
                                Ok(found) => {
                                    let group = found["log_group"].as_str().unwrap_or_default();
                                    aws.open_tail(host, &profile, title, group, None, cx);
                                }
                                Err(failure) => host.said(&failure, cx),
                            },
                            cx,
                        );
                    }));
                Some(detail(
                    service.name.clone(),
                    Some(service.arn.clone()),
                    vec![tail.into_any_element(), console(&service.console_url)],
                    vec![
                        fact("Status", service.state_label.clone()),
                        fact("Desired", service.desired.to_string()),
                        fact("Running", service.running.to_string()),
                        fact("Pending", service.pending.to_string()),
                        fact("Deployed", service.deployed_at.as_deref().map(ago)),
                    ],
                    vec![tasks],
                    cx,
                ))
            }
        };
        let back = IconButton::new("aws-ecs-back", Icon::Chevron, "Back to the clusters")
            .turned(std::f32::consts::PI)
            .small()
            .on_click(listen::<Aws, ClickEvent>(cx, |aws, _, _, _, cx| {
                aws.ecs = Ecs::Clusters;
                cx.notify();
            }));
        let list = self.list(
            "ecs-services",
            heading(
                vec!["ECS".into(), cluster.to_owned().into()],
                view.services.len(),
                Some(back.into_any_element()),
                cx,
            ),
            items,
            chosen.map(|s| s.name.as_str()),
            "This cluster runs no services.",
            cx,
        );
        Ok(two(list, detail, cx))
    }

    /// A service's tasks, each opening its own log stream.
    fn tasks(
        &self,
        host: &mut Host,
        profile: &str,
        cluster: &str,
        service: &str,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let params = json!({ "profile": profile, "cluster": cluster, "service": service });
        let tasks = match host.view("ecsTasksView", params, cx).read::<EcsTasks>() {
            Ok(Some(tasks)) => tasks,
            Ok(None) => return ui::meta("Loading tasks…", cx).into_any_element(),
            Err(failure) => return ui::meta(failure.message, cx).into_any_element(),
        };
        let rows = tasks.tasks.iter().enumerate().map(|(index, task)| {
            let (profile, cluster, arn, title) = (
                profile.to_owned(),
                cluster.to_owned(),
                task.arn.clone(),
                format!("{service} · {}", task.short_id),
            );
            let hover = t.surface_hover;
            div()
                .id(("aws-task", index))
                .px(SPACE_1H)
                .py(SPACE_1)
                .rounded(theme::RADIUS_2)
                .flex()
                .items_center()
                .gap(SPACE_2)
                .cursor_pointer()
                .hover(move |row| row.bg(hover))
                .on_click(listen::<Aws, ClickEvent>(cx, move |_, host, _, _, cx| {
                    let asked = host.call(
                        "ecsTaskLogConfig",
                        json!({ "profile": profile, "cluster": cluster, "taskArn": arn }),
                    );
                    let (profile, title) = (profile.clone(), title.clone());
                    after::<Aws, _>(
                        asked,
                        move |aws, host, answer, cx| match answer {
                            Ok(found) => {
                                let group = found["log_group"].as_str().unwrap_or_default();
                                let stream = found["log_stream"].as_str().map(str::to_owned);
                                aws.open_tail(host, &profile, title, group, stream, cx);
                            }
                            Err(failure) => host.said(&failure, cx),
                        },
                        cx,
                    );
                }))
                .child(ui::dot(tone(task.health), cx))
                .child(ui::mono(task.short_id.clone(), 11.5, cx).flex_1())
                .child(ui::meta(
                    [task.zone.clone(), task.started_at.as_deref().map(ago)]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" · "),
                    cx,
                ))
        });
        div()
            .flex()
            .flex_col()
            .gap(SPACE_0)
            .child(ui::caps("Tasks", cx))
            .when(tasks.tasks.is_empty(), |list| {
                list.child(ui::meta("No tasks running.", cx))
            })
            .children(rows)
            .into_any_element()
    }

    fn ec2(
        &mut self,
        host: &mut Host,
        profile: &str,
        cx: &mut Context<PluginPane>,
    ) -> Result<AnyElement, AnyElement> {
        let view: Rc<Ec2> = read(host, "ec2View", json!({ "profile": profile }), profile, cx)?;
        let chosen = self.chosen("ec2", &view.instances, |i| &i.id);
        let items = view
            .instances
            .iter()
            .map(|instance| Item {
                tone: Some(tone(instance.health)),
                sub: Some(instance.id.clone().into()),
                aside: Some(instance.state_label.clone().into()),
                dim: instance.idle,
                ..Item::new(
                    instance.id.clone(),
                    instance
                        .name
                        .clone()
                        .unwrap_or_else(|| "unnamed".to_owned()),
                )
            })
            .collect();
        let detail = chosen.map(|instance| {
            detail(
                instance
                    .name
                    .clone()
                    .unwrap_or_else(|| "unnamed".to_owned()),
                Some(instance.id.clone()),
                vec![console(&instance.console_url)],
                vec![
                    fact("State", instance.state_label.clone()),
                    copyable("Type", instance.instance_type.clone()),
                    copyable("Private IP", instance.private_ip.clone()),
                    copyable("Public IP", instance.public_ip.clone()),
                    fact("Launched", instance.launched_at.as_deref().map(ago)),
                    copyable("Instance ID", instance.id.clone()),
                ],
                Vec::new(),
                cx,
            )
        });
        let list = self.list(
            "ec2",
            heading(
                vec!["EC2".into(), format!("{} running", view.running).into()],
                view.instances.len(),
                None,
                cx,
            ),
            items,
            chosen.map(|i| i.id.as_str()),
            "No instances in this region.",
            cx,
        );
        Ok(two(list, detail, cx))
    }

    fn lambda(
        &mut self,
        host: &mut Host,
        profile: &str,
        cx: &mut Context<PluginPane>,
    ) -> Result<AnyElement, AnyElement> {
        let view: Rc<Lambda> = read(
            host,
            "lambdaView",
            json!({ "profile": profile }),
            profile,
            cx,
        )?;
        let chosen = self.chosen("lambda", &view.functions, |f| &f.name);
        let items = view
            .functions
            .iter()
            .map(|function| Item {
                tone: function.deprecated.then_some(Tone::Pending),
                sub: Some(
                    match (&function.runtime, function.deprecated) {
                        (Some(runtime), true) => format!("{runtime} · deprecated"),
                        (Some(runtime), false) => runtime.clone(),
                        (None, _) => "—".to_owned(),
                    }
                    .into(),
                ),
                aside: Some(function.memory_label.clone().into()),
                bar: Some(function.memory_ratio),
                ..Item::new(function.name.clone(), function.name.clone())
            })
            .collect();
        let detail = chosen.map(|function| {
            let (profile, group, title) = (
                profile.to_owned(),
                function.log_group.clone(),
                function.name.clone(),
            );
            let tail = Button::new("aws-lambda-tail", "Tail logs", Variant::Primary)
                .icon(Icon::LogLines)
                .on_click(listen::<Aws, ClickEvent>(cx, move |aws, host, _, _, cx| {
                    aws.open_tail(host, &profile, title.clone(), &group, None, cx);
                }));
            detail(
                function.name.clone(),
                Some(function.log_group.clone()),
                vec![tail.into_any_element(), console(&function.console_url)],
                vec![
                    fact("Memory", function.memory_label.clone()),
                    fact(
                        "Timeout",
                        function.timeout_secs.map(|secs| format!("{secs} s")),
                    ),
                    fact("Runtime", function.runtime.clone()),
                    copyable("Handler", function.handler.clone()),
                    fact("Modified", function.modified_at.as_deref().map(ago)),
                    copyable("Log group", function.log_group.clone()),
                ],
                Vec::new(),
                cx,
            )
        });
        let list = self.list(
            "lambda",
            heading(
                vec!["Lambda".into(), "Functions".into()],
                view.functions.len(),
                None,
                cx,
            ),
            items,
            chosen.map(|f| f.name.as_str()),
            "No functions in this region.",
            cx,
        );
        Ok(two(list, detail, cx))
    }

    fn sqs(
        &mut self,
        host: &mut Host,
        profile: &str,
        cx: &mut Context<PluginPane>,
    ) -> Result<AnyElement, AnyElement> {
        let view: Rc<Sqs> = read(host, "sqsView", json!({ "profile": profile }), profile, cx)?;
        let chosen = self.chosen("sqs", &view.queues, |q| &q.url);
        let count = |n: Option<u64>| n.map_or_else(|| "—".to_owned(), |n| n.to_string());
        let items = view
            .queues
            .iter()
            .map(|queue| {
                let kinds: Vec<&str> = [
                    queue.fifo.then_some("fifo"),
                    queue.dead_letter.then_some("dead-letter"),
                ]
                .into_iter()
                .flatten()
                .collect();
                Item {
                    tone: queue.needs_look.then_some(Tone::Pending),
                    sub: (!kinds.is_empty()).then(|| kinds.join(" · ").into()),
                    aside: Some(count(queue.available).into()),
                    bar: Some(queue.ratio),
                    ..Item::new(queue.url.clone(), queue.name.clone())
                }
            })
            .collect();
        let detail = chosen.map(|queue| {
            detail(
                queue.name.clone(),
                Some(queue.url.clone()),
                vec![console(&queue.console_url)],
                vec![
                    fact("Available", count(queue.available)),
                    fact("In flight", count(queue.in_flight)),
                    fact("Delayed", count(queue.delayed)),
                    fact(
                        "Type",
                        if queue.fifo { "FIFO" } else { "Standard" }.to_owned(),
                    ),
                    fact(
                        "Role",
                        if queue.dead_letter {
                            "Dead-letter queue"
                        } else {
                            "Queue"
                        }
                        .to_owned(),
                    ),
                    copyable("URL", queue.url.clone()),
                ],
                Vec::new(),
                cx,
            )
        });
        let list = self.list(
            "sqs",
            heading(
                vec![
                    "SQS".into(),
                    format!("{} dead-letter", view.dead_letters).into(),
                ],
                view.queues.len(),
                None,
                cx,
            ),
            items,
            chosen.map(|q| q.url.as_str()),
            "No queues in this region.",
            cx,
        );
        Ok(two(list, detail, cx))
    }

    fn s3(
        &mut self,
        host: &mut Host,
        profile: &str,
        cx: &mut Context<PluginPane>,
    ) -> Result<AnyElement, AnyElement> {
        let view: Rc<S3> = read(host, "s3View", json!({ "profile": profile }), profile, cx)?;
        let chosen = self.chosen("s3", &view.buckets, |b| &b.name);
        let items = view
            .buckets
            .iter()
            .map(|bucket| Item {
                aside: bucket.created_at.as_deref().map(|at| ago(at).into()),
                ..Item::new(bucket.name.clone(), bucket.name.clone())
            })
            .collect();
        let detail = chosen.map(|bucket| {
            detail(
                bucket.name.clone(),
                Some(bucket.arn.clone()),
                vec![console(&bucket.console_url)],
                vec![
                    copyable("URI", bucket.uri.clone()),
                    copyable("ARN", bucket.arn.clone()),
                    fact("Created", bucket.created_at.as_deref().map(ago)),
                ],
                Vec::new(),
                cx,
            )
        });
        let list = self.list(
            "s3",
            heading(
                vec!["S3".into(), "Buckets".into()],
                view.buckets.len(),
                None,
                cx,
            ),
            items,
            chosen.map(|b| b.name.as_str()),
            "This account has no buckets.",
            cx,
        );
        Ok(two(list, detail, cx))
    }

    fn billing(
        &mut self,
        host: &mut Host,
        profile: &str,
        cx: &mut Context<PluginPane>,
    ) -> Result<AnyElement, AnyElement> {
        let params = json!({ "profile": profile, "monthsBack": MONTHS_BACK });
        let view: Rc<Billing> = read(host, "billingView", params, profile, cx)?;
        let unit = view.unit.clone();
        let picked = self.picked.get("billing");
        let chosen: Option<&BillingMonth> = (view.months.iter())
            .find(|month| picked.is_some_and(|picked| month.period_start == *picked))
            .or_else(|| view.months.get(view.current));
        let items = (view.months.iter().rev())
            .map(|month| Item {
                sub: month.in_progress.then(|| match month.day {
                    Some(day) => format!("month to date · day {day} of {}", month.days).into(),
                    None => "month to date".into(),
                }),
                aside: Some(money(month.gross, &unit, 2).into()),
                bar: Some(month.ratio),
                ..Item::new(month.period_start.clone(), month.long_label.clone())
            })
            .collect();
        let summary = [
            Some(format!("Forecast {}", money(view.forecast, &unit, 0))),
            view.change_pct
                .map(|pct| format!("{pct:+.1}% on the last month")),
            view.average
                .map(|average| format!("average {}", money(average, &unit, 0))),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        let detail = chosen.map(|month| {
            let charges = month.charges.iter().map(|charge| {
                let change = match &charge.change {
                    Some(BillingChange::New) => "new".to_owned(),
                    Some(BillingChange::Flat) => "—".to_owned(),
                    Some(BillingChange::Up { pct } | BillingChange::Down { pct }) => {
                        format!("{pct:+.0}%")
                    }
                    None => String::new(),
                };
                bar_row(
                    charge.short_name.clone(),
                    format!("{change}  {}", money(charge.amount, &unit, 2)),
                    charge.ratio,
                    cx,
                )
            });
            let credits = month
                .credit_lines
                .iter()
                .map(|credit| fact_row(&credit.service, &money(credit.amount, &unit, 2), cx));
            let below = div()
                .flex()
                .flex_col()
                .gap(SPACE_1)
                .child(ui::caps(
                    &match &month.compared_to {
                        Some(before) => format!("By service, against {before}"),
                        None => "By service".to_owned(),
                    },
                    cx,
                ))
                .when(month.charges.is_empty(), |list| {
                    list.child(ui::meta("No charges this month.", cx))
                })
                .children(charges)
                .children(credits)
                .into_any_element();
            detail(
                month.long_label.clone(),
                None,
                Vec::new(),
                vec![
                    fact("Charges", money(month.gross, &unit, 2)),
                    fact("Credits", money(month.credits, &unit, 2)),
                    fact("Net", money(month.net, &unit, 2)),
                ],
                vec![below],
                cx,
            )
        });
        let list = self.list(
            "billing",
            div()
                .flex()
                .flex_col()
                .gap(SPACE_0)
                .child(heading(
                    vec!["Billing".into(), "Costs".into()],
                    view.months.len(),
                    None,
                    cx,
                ))
                .child(ui::meta(summary, cx).truncate())
                .into_any_element(),
            items,
            chosen.map(|month| month.period_start.as_str()),
            "No billing data for this account.",
            cx,
        );
        Ok(two(list, detail, cx))
    }

    fn list(
        &self,
        name: &'static str,
        heading: AnyElement,
        items: Vec<Item>,
        chosen: Option<&str>,
        empty: &'static str,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let body = match items.is_empty() {
            true => super::empty(empty, cx),
            false => div()
                .id(name)
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(SPACE_1H)
                .pb(SPACE_2)
                .flex()
                .flex_col()
                .gap(px(1.))
                .children(items.into_iter().enumerate().map(|(index, item)| {
                    let open = chosen == Some(item.key.as_str());
                    row(name, index, item, open, cx)
                }))
                .into_any_element(),
        };
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(SPACE_3H)
                    .py(SPACE_2H)
                    .overflow_hidden()
                    .border_b_1()
                    .border_color(t.border)
                    .child(heading),
            )
            .child(body)
            .into_any_element()
    }

    fn open_tail(
        &mut self,
        host: &Host,
        profile: &str,
        title: String,
        group: &str,
        stream: Option<String>,
        cx: &mut Context<PluginPane>,
    ) {
        let reading = host.stream::<Aws>(
            "tailLogs",
            json!({ "profile": profile, "logGroup": group, "logStream": stream, "since": "5m" }),
            cx,
            |aws, _, streamed, _| {
                if let Some(tail) = &mut aws.tail {
                    tail.take(streamed);
                }
            },
        );
        self.tail = Some(Tail {
            title,
            group: group.to_owned(),
            stream,
            lines: Rc::default(),
            live: true,
            failed: None,
            level: 0,
            follow: true,
            scroll: UniformListScrollHandle::new(),
            _reading: reading,
        });
        cx.notify();
    }

    fn tail_page(&self, cx: &mut Context<PluginPane>) -> AnyElement {
        let Some(tail) = &self.tail else {
            return div().into_any_element();
        };
        let t = Tokens::get(cx).clone();
        let errors = (tail.lines.iter())
            .filter(|line| line.level == Some(LogLevel::Error))
            .count();
        let warnings = (tail.lines.iter())
            .filter(|line| line.level == Some(LogLevel::Warn))
            .count();
        let shown: Rc<Vec<usize>> = Rc::new(
            (tail.lines.iter().enumerate())
                .filter(|(_, line)| tail.shows(line))
                .map(|(at, _)| at)
                .collect(),
        );
        let back = IconButton::new("aws-tail-back", Icon::Chevron, "Back to the list")
            .turned(std::f32::consts::PI)
            .small()
            .on_click(listen::<Aws, ClickEvent>(cx, |aws, _, _, _, cx| {
                aws.tail = None;
                cx.notify();
            }));
        let levels = Segmented::words(
            "aws-tail-level",
            vec![
                ("All".into(), None),
                ("Warnings".into(), Some((warnings + errors) as u64)),
                ("Errors".into(), Some(errors as u64)),
            ],
            tail.level,
        )
        .on_select(listen::<Aws, usize>(cx, |aws, _, level, _, cx| {
            if let Some(tail) = &mut aws.tail {
                tail.level = *level;
            }
            cx.notify();
        }));
        let follow =
            Toggle::new("aws-tail-follow", "Follow", tail.follow).on_click(
                listen::<Aws, ClickEvent>(cx, |aws, _, _, _, cx| {
                    if let Some(tail) = &mut aws.tail {
                        tail.follow = !tail.follow;
                    }
                    cx.notify();
                }),
            );
        let where_from = match &tail.stream {
            Some(stream) => format!("{} · {stream}", tail.group),
            None => tail.group.clone(),
        };
        let body = match (&tail.failed, shown.is_empty()) {
            (Some(failed), _) => super::empty(failed.clone(), cx),
            (None, true) if tail.lines.is_empty() => {
                super::empty("No events in the last 5 minutes. Waiting for new ones…", cx)
            }
            (None, true) => super::empty("No lines at this level.", cx),
            (None, false) => {
                let (lines, ink) = (tail.lines.clone(), t.clone());
                uniform_list("aws-tail-lines", shown.len(), move |range, _, cx| {
                    range
                        .filter_map(|at| shown.get(at).and_then(|line| lines.get(*line)))
                        .map(|line| log_line(line, &ink, cx))
                        .collect()
                })
                .track_scroll(&tail.scroll)
                .flex_1()
                .into_any_element()
            }
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(SPACE_2H)
                    .py(SPACE_2)
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .border_b_1()
                    .border_color(t.border)
                    .child(back)
                    .child(ui::text(tail.title.clone(), 13., FontWeight::SEMIBOLD))
                    .child(ui::mono(where_from, 11., cx).min_w_0().truncate().flex_1())
                    .child(ui::meta(if tail.live { "Tailing" } else { "Ended" }, cx))
                    .child(levels)
                    .child(follow),
            )
            .child(div().flex_1().min_h_0().flex().flex_col().child(body))
            .child(
                div()
                    .px(SPACE_3)
                    .py(SPACE_1H)
                    .border_t_1()
                    .border_color(t.border)
                    .child(ui::meta(
                        format!(
                            "{} lines · {errors} errors · {warnings} warnings",
                            tail.lines.len()
                        ),
                        cx,
                    )),
            )
            .into_any_element()
    }
}

/// Reads view `method`, standing in what the pane shows instead while it loads or fails.
fn read<T: DeserializeOwned + 'static>(
    host: &mut Host,
    method: &str,
    params: Value,
    profile: &str,
    cx: &mut Context<PluginPane>,
) -> Result<Rc<T>, AnyElement> {
    match host.view(method, params, cx).read::<T>() {
        Ok(Some(view)) => Ok(view),
        Ok(None) => Err(super::loading(cx)),
        Err(failure) if lapsed(&failure) => Err(sign_in(host, profile, cx)),
        Err(failure) => Err(super::failure(&failure, cx)),
    }
}

/// The profile's sign-in ran out, or it has none.
fn lapsed(failure: &Failure) -> bool {
    failure.signed_out()
        || matches!(
            failure.category.as_ref(),
            "aws-token-expired" | "aws-no-credentials"
        )
}

fn tone(health: Health) -> Tone {
    match health {
        Health::Ok => Tone::Ok,
        Health::Warn => Tone::Pending,
        Health::Fail => Tone::Failed,
        Health::Off => Tone::Idle,
    }
}

/// The account view as the plugin sends it: the profiles, each with how it signs in.
#[derive(serde::Deserialize)]
struct AccountView {
    profile: Option<String>,
    profiles: Vec<ProfileView>,
}

#[derive(serde::Deserialize)]
struct ProfileView {
    #[serde(flatten)]
    profile: sikemux_plugin_aws_views::AccountProfile,
    #[serde(default)]
    kind: Option<String>,
}

impl AccountView {
    fn account(&self) -> Account {
        Account {
            profile: self.profile.clone(),
            profiles: self
                .profiles
                .iter()
                .map(|each| each.profile.clone())
                .collect(),
        }
    }

    fn kinds(&self) -> HashMap<String, String> {
        self.profiles
            .iter()
            .filter_map(|each| Some((each.profile.name.clone(), each.kind.clone()?)))
            .collect()
    }
}

/// The profile in use, as the Mac's AWS rail ends: its name and account, its region and
/// whether it is signed in, and a menu of the profiles to switch to.
fn account_switch(
    account: &Account,
    kinds: &HashMap<String, String>,
    profile: &str,
    signed_in: Option<bool>,
    cx: &mut Context<PluginPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let current = account.profiles.iter().find(|each| each.name == profile);
    let title = current.map_or_else(|| profile.to_owned(), |each| each.title.clone());
    let id = current
        .and_then(|each| each.account.clone())
        .unwrap_or_else(|| "—".to_owned());
    let region = current
        .and_then(|each| each.region.clone())
        .unwrap_or_else(|| "no region".to_owned());
    let (tone, said) = match signed_in {
        Some(true) => (Tone::Ok, "Signed in"),
        Some(false) => (Tone::Idle, "Signed out"),
        None => (Tone::Pending, "Checking"),
    };
    let (profiles, chosen, pane) = (account.profiles.clone(), profile.to_owned(), cx.entity());
    let kinds = kinds.clone();
    KitButton::new("aws-account")
        .ghost()
        .w_full()
        .h_auto()
        .py(SPACE_1H)
        .px(SPACE_2)
        .rounded(theme::RADIUS_2)
        .tooltip(format!("{profile} · switch profile"))
        .child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(SPACE_0)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_1H)
                        .child(
                            ui::text(title, 12.5, FontWeight::MEDIUM)
                                .text_color(t.text_primary)
                                .min_w_0()
                                .truncate(),
                        )
                        .child(ui::mono(id, 11., cx).text_color(t.text_tertiary))
                        .child(div().flex_1())
                        .child(ui::icon(Icon::Chevron, 11.).text_color(t.text_tertiary)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_1H)
                        .text_size(theme::ui_text(TEXT_2XS))
                        .text_color(t.text_tertiary)
                        .child(region)
                        .child(div().flex_1())
                        .child(ui::dot(tone, cx))
                        .child(said),
                ),
        )
        .dropdown_menu_with_anchor(
            gpui::Anchor::BottomLeft,
            ui::menu(move |mut menu, _, cx| {
                let t = Tokens::get(cx).clone();
                menu = menu
                    .min_w(px(240.))
                    .check_side(gpui_kit::component::Side::Right)
                    .label("Profiles");
                for each in &profiles {
                    let on = each.name == chosen;
                    let kind = kinds.get(&each.name).map(|kind| match kind.as_str() {
                        "sso" => "SSO".to_owned(),
                        other => other.to_owned(),
                    });
                    let meta = [each.account.clone(), kind, each.region.clone()]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" · ");
                    let (title, name, pane) = (each.title.clone(), each.name.clone(), pane.clone());
                    let picked = name.clone();
                    menu = menu.item(
                        PopupMenuItem::element(move |_, _| {
                            div()
                                .id(SharedString::from(format!("aws-profile-{name}")))
                                .aria_label(title.clone())
                                .test_support()
                                .flex()
                                .flex_col()
                                .child(title.clone())
                                .when(!meta.is_empty(), |item| {
                                    item.child(
                                        div()
                                            .text_size(theme::ui_text(TEXT_2XS))
                                            .text_color(t.text_tertiary)
                                            .child(meta.clone()),
                                    )
                                })
                        })
                        .icon(Icon::User)
                        .checked(on)
                        .on_click(move |_, _, cx| {
                            if !on {
                                let params = json!({ "profile": picked });
                                pane.update(cx, |pane, cx| {
                                    write(pane, "setDefaultAccount", params, cx)
                                });
                            }
                        }),
                    );
                }
                menu
            }),
        )
        .into_any_element()
}

fn choose_profile(account: &Account, cx: &mut Context<PluginPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let rows = account.profiles.iter().enumerate().map(|(index, each)| {
        let name = each.name.clone();
        let hover = t.surface_hover;
        div()
            .id(("aws-pick-profile", index))
            .w(px(320.))
            .px(SPACE_2H)
            .py(px(7.))
            .rounded(theme::RADIUS_2)
            .flex()
            .items_center()
            .gap(SPACE_2)
            .cursor_pointer()
            .hover(move |row| row.bg(hover))
            .on_click(cx.listener(move |pane, _, _, cx| {
                write(pane, "setDefaultAccount", json!({ "profile": name }), cx)
            }))
            .child(ui::text(each.title.clone(), 12.5, FontWeight::MEDIUM).flex_1())
            .children(each.account.clone().map(|id| ui::mono(id, 11., cx)))
    });
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(SPACE_1H)
        .child(
            ui::text("Pick the AWS profile to show", 14., FontWeight::SEMIBOLD)
                .text_color(t.text_primary),
        )
        .child(ui::text(PITCH, 12., FontWeight::NORMAL).text_color(t.text_tertiary))
        .when(account.profiles.is_empty(), |page| {
            page.child(ui::meta(
                "No profiles in ~/.aws/config. Run aws configure sso to add one.",
                cx,
            ))
        })
        .children(rows)
        .into_any_element()
}

fn sign_in(host: &Host, profile: &str, cx: &mut Context<PluginPane>) -> AnyElement {
    let named = profile.to_owned();
    let ink = Tokens::get(cx).text_primary;
    ui::sign_in::Screen::new(
        "aws-sign-in-page",
        ui::picture(Icon::Aws, 28., ink),
        format!("Sign in to {profile}"),
    )
    .lede("Its AWS sign-in has run out.")
    .child(super::browser_sign_in(
        host,
        "aws-sign-in",
        Icon::LogIn,
        "Sign in with the browser",
        move |host, cx| host.cloud_sign_in("signIn", json!({ "profile": named }), cx),
        cx,
    ))
    .into_any_element()
}

/// The profile agents start on in `project`, or in every project not picked for.
fn agent_pick(
    id: &'static str,
    picks: &AgentProfiles,
    picked: Option<String>,
    unset: String,
    project: Option<String>,
    cx: &mut Context<PluginPane>,
) -> AnyElement {
    let unset: SharedString = unset.into();
    let options = std::iter::once(Pick::new("", unset.clone()))
        .chain((picks.profiles.iter()).map(|name| Pick::new(name.clone(), name.clone())))
        .collect();
    let pane = cx.entity();
    let field = ui::select(
        id,
        picked.clone().map_or(unset, Into::into),
        options,
        Some(picked.unwrap_or_default().into()),
        false,
        move |key, _, cx| {
            let profile = (!key.is_empty()).then(|| key.to_string());
            let params = json!({ "project": project, "profile": profile });
            pane.update(cx, |pane, cx| write(pane, "pickAgentProfile", params, cx));
        },
        cx,
    );
    div().px(SPACE_0).child(field).into_any_element()
}

/// Calls the plugin's `method`, saying over the window when it fails; the core reads the views
/// again after it.
fn write(pane: &PluginPane, method: &'static str, params: Value, cx: &mut Context<PluginPane>) {
    let call = pane.host.call(method, params);
    cx.spawn(async move |pane, cx| {
        if let Err(failure) = call.await {
            let _ = pane.update(cx, |pane, cx| pane.host.said(&failure, cx));
        }
    })
    .detach();
}

/// Where the list is, as `ECS › Clusters`, and how many it holds.
fn heading(
    trail: Vec<SharedString>,
    count: usize,
    back: Option<AnyElement>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let last = trail.len().saturating_sub(1);
    div()
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .children(back)
        .children(trail.into_iter().enumerate().map(|(at, part)| {
            let ink = if at == last {
                t.text_primary
            } else {
                t.text_tertiary
            };
            div()
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .min_w_0()
                .when(at > 0, |part| part.child(ui::meta("›", cx)))
                .child(
                    ui::text(part, 13., FontWeight::SEMIBOLD)
                        .text_color(ink)
                        .min_w_0()
                        .truncate(),
                )
        }))
        .child(ui::meta(count.to_string(), cx))
        .into_any_element()
}

fn row(
    list: &'static str,
    index: usize,
    item: Item,
    open: bool,
    cx: &mut Context<PluginPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let hover = t.surface_hover;
    let key = item.key;
    div()
        .id((list, index))
        .px(SPACE_2)
        .py(SPACE_1H)
        .rounded(theme::RADIUS_2)
        .flex()
        .flex_col()
        .gap(px(3.))
        .cursor_pointer()
        .when(open, |row| row.bg(t.surface_chosen))
        .when(!open, |row| row.hover(move |row| row.bg(hover)))
        .when(item.dim, |row| row.opacity(0.55))
        .on_click(listen::<Aws, ClickEvent>(cx, move |aws, _, _, _, cx| {
            aws.picked.insert(list, key.clone());
            cx.notify();
        }))
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .children(item.tone.map(|tone| ui::dot(tone, cx)))
                .child(
                    ui::text(item.title, 12.5, FontWeight::MEDIUM)
                        .text_color(t.text_primary)
                        .flex_1()
                        .min_w_0()
                        .truncate(),
                )
                .children(item.aside.map(|aside| ui::mono(aside, 11., cx).flex_none())),
        )
        .children(item.sub.map(|sub| ui::meta(sub, cx).truncate()))
        .children(item.bar.map(|ratio| bar(ratio, &t)))
        .into_any_element()
}

/// The row picked: its name, what to do with it, and its facts.
fn detail(
    title: impl Into<SharedString>,
    sub: Option<String>,
    actions: Vec<AnyElement>,
    facts: Vec<Fact>,
    below: Vec<AnyElement>,
    cx: &mut Context<PluginPane>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let facts = facts.into_iter().enumerate().map(|(index, fact)| {
        let value = fact.value.unwrap_or_else(|| "—".to_owned());
        let copy = fact.copy.then(|| {
            let (text, what) = (value.clone(), fact.label.to_lowercase());
            IconButton::new(("aws-copy", index), Icon::Copy, format!("Copy the {what}"))
                .small()
                .on_click(listen::<Aws, ClickEvent>(cx, move |_, host, _, _, cx| {
                    host.copy(text.clone(), &what, cx)
                }))
        });
        div()
            .flex()
            .items_center()
            .gap(SPACE_2)
            .min_h(px(22.))
            .child(ui::meta(fact.label, cx).w(px(96.)).flex_none())
            .child(
                ui::text(value, 12., FontWeight::NORMAL)
                    .text_color(t.text_primary)
                    .flex_1()
                    .min_w_0()
                    .truncate(),
            )
            .children(copy)
    });
    div()
        .id("aws-detail")
        .size_full()
        .overflow_y_scroll()
        .px(SPACE_4)
        .py(SPACE_3)
        .flex()
        .flex_col()
        .gap(SPACE_3)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(SPACE_0)
                .child(
                    ui::text(title, 14., FontWeight::SEMIBOLD)
                        .text_color(t.text_primary)
                        .truncate(),
                )
                .children(sub.map(|sub| ui::mono(sub, 11., cx).truncate())),
        )
        .when(!actions.is_empty(), |page| {
            page.child(div().flex().flex_wrap().gap(SPACE_1H).children(actions))
        })
        .child(div().flex().flex_col().children(facts))
        .children(below)
        .into_any_element()
}

/// The list beside the detail of the row picked in it.
fn two(list: AnyElement, detail: Option<AnyElement>, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .size_full()
        .flex()
        .child(list)
        .child(
            div()
                .w(px(320.))
                .flex_none()
                .h_full()
                .border_l_1()
                .border_color(t.border)
                .child(detail.unwrap_or_else(|| super::empty("Nothing picked.", cx))),
        )
        .into_any_element()
}

fn console(url: &str) -> AnyElement {
    let url = url.to_owned();
    IconButton::new("aws-console", Icon::Globe, "Open in the AWS console")
        .small()
        .on_click(move |_, _, cx| cx.open_url(&url))
        .into_any_element()
}

fn bar(ratio: f64, t: &Tokens) -> Div {
    let share = ratio.clamp(0., 1.) as f32;
    div()
        .h(px(3.))
        .w_full()
        .rounded_full()
        .bg(t.surface_hover)
        .child(
            div()
                .h_full()
                .w(gpui_kit::relative(share))
                .rounded_full()
                .bg(t.accent),
        )
}

fn bar_row(name: String, amount: String, ratio: f64, cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .py(px(3.))
        .child(
            div()
                .flex()
                .gap(SPACE_2)
                .child(
                    ui::text(name, 12., FontWeight::NORMAL)
                        .text_color(t.text_primary)
                        .flex_1()
                        .min_w_0()
                        .truncate(),
                )
                .child(ui::mono(amount, 11., cx)),
        )
        .child(bar(ratio, t))
}

fn fact_row(name: &str, amount: &str, cx: &App) -> Div {
    div()
        .flex()
        .gap(SPACE_2)
        .py(px(3.))
        .child(ui::meta(name.to_owned(), cx).flex_1().min_w_0().truncate())
        .child(ui::mono(amount.to_owned(), 11., cx))
}

fn log_line(line: &LogLine, t: &Tokens, cx: &App) -> AnyElement {
    let ink = match line.level {
        Some(LogLevel::Error) => t.danger,
        Some(LogLevel::Warn) => t.warn,
        None => t.text_primary,
    };
    div()
        .h(px(LINE))
        .px(SPACE_3)
        .flex()
        .items_center()
        .gap(SPACE_2H)
        .child(
            ui::mono(line.clock.clone().unwrap_or_default(), 11., cx)
                .w(px(60.))
                .flex_none(),
        )
        .child(
            ui::mono(line.text.clone(), 11.5, cx)
                .text_color(ink)
                .min_w_0()
                .truncate(),
        )
        .into_any_element()
}

/// `$1,843.20`, or `EUR 1,843.20` in another currency.
fn money(amount: f64, unit: &str, digits: usize) -> String {
    let amount = if amount.abs() < 0.005 { 0. } else { amount };
    let body = format!("{:.*}", digits, amount.abs());
    let (whole, cents) = body.split_once('.').unwrap_or((&body, ""));
    let mut grouped = String::new();
    for (at, digit) in whole.chars().enumerate() {
        if at > 0 && (whole.len() - at) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    let sign = if amount < 0. { "-" } else { "" };
    let symbol = match unit {
        "USD" => "$".to_owned(),
        other => format!("{other} "),
    };
    match cents.is_empty() {
        true => format!("{sign}{symbol}{grouped}"),
        false => format!("{sign}{symbol}{grouped}.{cents}"),
    }
}
