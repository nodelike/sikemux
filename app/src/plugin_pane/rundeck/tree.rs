//! The projects' tree down the side: each project and its folders.

use super::*;

use crate::theme::{SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3};

impl Rundeck {
    pub(super) fn tree(
        &self,
        host: &mut Host,
        tree: Result<Option<Rc<Tree>>, Failure>,
        live: &HashSet<String>,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let t = Tokens::get(cx).clone();
        let body = match tree {
            Ok(None) => crate::plugin_pane::loading(cx),
            Err(failure) => crate::plugin_pane::failure(&failure, cx),
            Ok(Some(tree)) if tree.projects.is_empty() => {
                crate::plugin_pane::empty("This sign-in sees no projects.", cx)
            }
            Ok(Some(tree)) => {
                let mut rows = Vec::new();
                for project in &tree.projects {
                    let open = self.open.contains(&project.name);
                    let chosen = self.stack.is_empty()
                        && self.project == project.name
                        && self.group.is_none();
                    let lit = self.project == project.name && live.contains("");
                    let name = project.name.clone();
                    rows.push(
                        tree_row(
                            ("rundeck-project", rows.len()),
                            &project.name,
                            0,
                            (project.total > 0).then_some(open),
                            chosen,
                            lit,
                            project.error.is_none().then_some(project.total),
                            cx,
                        )
                        .on_click(listen::<Rundeck, ClickEvent>(cx, move |rd, _, _, _, cx| {
                            rd.pick(&name, None);
                            cx.notify();
                        }))
                        .into_any_element(),
                    );
                    if !open {
                        continue;
                    }
                    let project_name = project.name.clone();
                    let in_activity = matches!(
                        self.stack.last(),
                        Some(Level::Activity { project }) if *project == project_name
                    );
                    rows.push(
                        tree_row(
                            ("rundeck-activity", rows.len()),
                            "Activity",
                            1,
                            None,
                            in_activity,
                            false,
                            None,
                            cx,
                        )
                        .on_click(listen::<Rundeck, ClickEvent>(cx, move |rd, _, _, _, cx| {
                            rd.stack = vec![Level::Activity {
                                project: project_name.clone(),
                            }];
                            cx.notify();
                        }))
                        .into_any_element(),
                    );
                    if let Some(error) = &project.error {
                        rows.push(
                            div()
                                .pl(px(28.))
                                .py(SPACE_1)
                                .child(ui::meta(error.clone(), cx).text_color(t.danger))
                                .into_any_element(),
                        );
                    }
                    self.folders(&project.name, &project.groups, 1, live, &mut rows, cx);
                }
                div()
                    .id("rundeck-tree")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(SPACE_1H)
                    .py(SPACE_2)
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .children(rows)
                    .into_any_element()
            }
        };
        div()
            .w(px(TREE_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(t.border)
            .child(
                div()
                    .px(SPACE_3)
                    .pt(SPACE_3)
                    .pb(SPACE_1)
                    .child(ui::caps("Projects", cx)),
            )
            .child(body)
            .child(connection(host, cx))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn folders(
        &self,
        project: &str,
        nodes: &[GroupNode],
        depth: usize,
        live: &HashSet<String>,
        rows: &mut Vec<AnyElement>,
        cx: &mut Context<PluginPane>,
    ) {
        for node in nodes {
            let key = format!("{project}/{}", node.path);
            let open = self.open.contains(&key);
            let chosen = self.stack.is_empty()
                && self.project == project
                && self.group.as_deref() == Some(node.path.as_str());
            let lit = self.project == project && live.contains(&node.path);
            let (project_name, path) = (project.to_owned(), node.path.clone());
            rows.push(
                tree_row(
                    ("rundeck-folder", rows.len()),
                    &node.name,
                    depth,
                    (!node.children.is_empty()).then_some(open),
                    chosen,
                    lit,
                    Some(node.count),
                    cx,
                )
                .on_click(listen::<Rundeck, ClickEvent>(cx, move |rd, _, _, _, cx| {
                    rd.pick(&project_name, Some(path.clone()));
                    cx.notify();
                }))
                .into_any_element(),
            );
            if open {
                self.folders(project, &node.children, depth + 1, live, rows, cx);
            }
        }
    }

    /// Shows a project's or a folder's jobs, opening it and the folders above it in the tree.
    pub(super) fn pick(&mut self, project: &str, group: Option<String>) {
        let same = self.stack.is_empty() && self.project == project && self.group == group;
        let key = match &group {
            Some(path) => format!("{project}/{path}"),
            None => project.to_owned(),
        };
        if same && self.open.contains(&key) {
            self.open.remove(&key);
        } else {
            self.open.insert(project.to_owned());
            if let Some(path) = &group {
                let mut above = String::new();
                for segment in path.split('/') {
                    if !above.is_empty() {
                        above.push('/');
                    }
                    above.push_str(segment);
                    self.open.insert(format!("{project}/{above}"));
                }
            }
        }
        self.project = project.to_owned();
        self.group = group;
        self.stack.clear();
    }

    pub(super) fn push(&mut self, level: Level) {
        self.stack.push(level);
    }
}

#[allow(clippy::too_many_arguments)]
fn tree_row(
    id: (&'static str, usize),
    name: &str,
    depth: usize,
    open: Option<bool>,
    chosen: bool,
    live: bool,
    count: Option<u32>,
    cx: &App,
) -> gpui_kit::Stateful<Div> {
    let t = Tokens::get(cx).clone();
    let hover = t.surface_hover;
    let chevron = match open {
        Some(open) => ui::icon(Icon::Chevron, 10.)
            .text_color(t.text_tertiary)
            .when(open, |icon| {
                icon.with_transformation(gpui_kit::Transformation::rotate(gpui_kit::radians(
                    std::f32::consts::FRAC_PI_2,
                )))
            })
            .into_any_element(),
        None => div().size(px(10.)).into_any_element(),
    };
    let glyph = if name == "Activity" && depth == 1 {
        Icon::Activity
    } else {
        Icon::Folder
    };
    div()
        .id(id)
        .pl(px(6. + depth as f32 * 14.))
        .pr(SPACE_2)
        .py(SPACE_1)
        .rounded(theme::RADIUS_2)
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .cursor_pointer()
        .when(chosen, |row| row.bg(t.surface_chosen))
        .when(!chosen, |row| row.hover(move |row| row.bg(hover)))
        .child(chevron)
        .child(ui::icon(glyph, 13.).text_color(t.text_tertiary))
        .child(
            ui::text(name.to_owned(), 12.5, FontWeight::NORMAL)
                .text_color(t.text_primary)
                .flex_1()
                .min_w_0()
                .truncate(),
        )
        .when(live, |row| {
            row.child(div().size(px(6.)).rounded_full().bg(t.warn))
        })
        .children(count.map(|count| ui::meta(count.to_string(), cx)))
}

/// Where the pane is signed in, and signing out.
fn connection(host: &mut Host, cx: &mut Context<PluginPane>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let status = host.status(cx).read::<Status>().ok().flatten();
    let place = status
        .as_ref()
        .map(|status| {
            status
                .details
                .url
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .trim_end_matches('/')
                .to_owned()
        })
        .unwrap_or_default();
    let user = status
        .as_ref()
        .map(|status| status.details.user.clone())
        .unwrap_or_default();
    let ok = status.as_ref().is_some_and(|status| status.ok);
    div()
        .border_t_1()
        .border_color(t.border)
        .px(SPACE_3)
        .py(SPACE_2H)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(ui::dot(if ok { ui::Tone::Ok } else { ui::Tone::Idle }, cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    ui::text(place, 12., FontWeight::MEDIUM)
                        .text_color(t.text_primary)
                        .truncate(),
                )
                .child(ui::meta(user, cx)),
        )
        .child(
            IconButton::new("rundeck-sign-out", Icon::LogOut, "Sign out")
                .small()
                .on_click(listen::<Rundeck, ClickEvent>(cx, |_, host, _, _, cx| {
                    let out = host.call("logout", Value::Null);
                    after::<Rundeck, _>(
                        out,
                        |_, host, done, cx| {
                            if let Err(failure) = done {
                                host.said(&failure.titled("Sign out of Rundeck"), cx);
                            }
                        },
                        cx,
                    );
                })),
        )
        .into_any_element()
}
