//! The deploy chip in a project's top bar: what the checkout's repository last deployed, and where.

use super::*;

use crate::theme::SPACE_1H;

/// The top bar's word on where the project in front deploys, and a menu to pick the place,
/// deploy there or check out what runs there.
#[derive(Default)]
pub struct DeployChip {
    /// The pointer has been over the top bar, so asking the deploy server is worth it.
    looked: bool,
    /// The job picked from the menu, for each project.
    picked: HashMap<String, (String, String)>,
    /// A checkout of the deployed branch is under way.
    checking_out: bool,
}

impl Panel for DeployChip {
    fn render(
        &mut self,
        host: &mut Host,
        _: &mut Window,
        cx: &mut Context<PluginPane>,
    ) -> AnyElement {
        let nothing = || div().into_any_element();
        if host.signed_in(cx) != Some(true) {
            return nothing();
        }
        let Some(project) = host.project(cx) else {
            return nothing();
        };
        self.looked |= host.hovered(cx);
        if !self.looked {
            return nothing();
        }
        let folder = project
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_owned();
        let picked = self.picked.get(&project).map(|(project, job_id)| Picked {
            project: project.clone(),
            job_id: job_id.clone(),
        });
        let target = host
            .view("deployTargetView", TargetRequest { folder, picked }, cx)
            .read::<DeployTarget>();
        let Ok(Some(target)) = target else {
            return nothing();
        };
        let Some(active) = target.active.clone() else {
            return nothing();
        };
        let t = Tokens::get(cx).clone();
        let dot = |tone: EnvTone| match tone {
            EnvTone::Prod => t.danger,
            EnvTone::Staging => t.warn,
            EnvTone::Preprod => t.cmd,
            EnvTone::Dev => t.live,
            EnvTone::Other => t.text_tertiary,
        };
        let pane = cx.weak_entity();
        let menu_target = target.clone();
        let repo = project.clone();
        let read = host.current_branch(cx);
        let checkout = read.clone().flatten();
        let checking_out = self.checking_out;
        KitButton::new("rundeck-deploy-chip")
            .ghost()
            .compact()
            .h(px(24.))
            .px(SPACE_1H)
            .gap(SPACE_1H)
            .rounded(theme::RADIUS_1)
            .tooltip(target.summary.clone())
            .child(ui::icon(Icon::Rundeck, 12.).text_color(t.text_tertiary))
            .child(div().size(px(6.)).rounded_full().bg(dot(active.tone)))
            .child(
                ui::text(active.place.clone(), 11.5, FontWeight::NORMAL)
                    .text_color(t.text_primary)
                    .max_w(px(260.))
                    .truncate(),
            )
            .dropdown_menu_with_anchor(
                gpui::Anchor::TopRight,
                ui::menu(move |popup, _, _| {
                    let target = &menu_target;
                    let mut popup = popup.min_w(px(280.));
                    if target.jobs.len() > 1 {
                        popup = popup.label("Deploy to");
                        for job in &target.jobs {
                            let (pane, project, picked) = (
                                pane.clone(),
                                repo.clone(),
                                (job.project.clone(), job.job_id.clone()),
                            );
                            popup = popup.item(
                                PopupMenuItem::new(job.place.clone())
                                    .checked(
                                        Some(&job.job_id)
                                            == target.active.as_ref().map(|active| &active.job_id),
                                    )
                                    .on_click(move |_, _, cx| {
                                        let _ = pane.update(cx, |pane, cx| {
                                            if let Some((chip, _)) = pane.parts::<DeployChip>() {
                                                chip.picked.insert(project.clone(), picked.clone());
                                            }
                                            cx.notify();
                                        });
                                    }),
                            );
                        }
                        popup = popup.separator();
                    }
                    let Some(active) = &target.active else {
                        return popup;
                    };
                    let job = JobRef {
                        project: active.project.clone(),
                        job_id: active.job_id.clone(),
                        name: active.name.clone(),
                        group: active.group.clone(),
                    };
                    let deploy_label = match (target.runs_branch, &read) {
                        (false, _) => format!("Run on {}", active.place),
                        (true, None) => "Deploy (reading the branch…)".to_owned(),
                        (true, Some(None)) => "Deploy (detached HEAD)".to_owned(),
                        (true, Some(Some(branch))) => format!("Deploy {branch}"),
                    };
                    let reading = target.runs_branch && read.is_none();
                    let at = place_key(
                        &job,
                        Some(checkout.as_deref().filter(|_| target.runs_branch)),
                    );
                    let opener = pane.clone();
                    popup = popup.item(
                        PopupMenuItem::new(deploy_label)
                            .icon(Icon::Rundeck)
                            .disabled(reading)
                            .on_click(move |_, _, cx| {
                                let _ = opener
                                    .update(cx, |pane, cx| pane.host.open_pane_at(at.clone(), cx));
                            }),
                    );
                    if let Some(deployed) = target.deployed_branch.clone() {
                        let (pane, repo) = (pane.clone(), repo.clone());
                        let here = checkout.as_deref() == Some(deployed.as_str());
                        let label = match checking_out {
                            true => format!("Checking out… {deployed}"),
                            false => format!("Check out {deployed}"),
                        };
                        popup = popup.item(
                            PopupMenuItem::new(label)
                                .icon(Icon::Checkout)
                                .disabled(here || checking_out)
                                .on_click(move |_, _, cx| {
                                    let _ = pane.update(cx, |pane, cx| {
                                        if let Some((chip, _)) = pane.parts::<DeployChip>() {
                                            chip.checking_out = true;
                                        }
                                        let checked = pane.host.checkout_branch(&repo, &deployed);
                                        crate::plugin_pane::after::<DeployChip, _>(
                                            checked,
                                            |chip, host, done, cx| {
                                                chip.checking_out = false;
                                                if let Err(failure) = done {
                                                    host.said(&failure.titled("Check out"), cx);
                                                }
                                            },
                                            cx,
                                        );
                                    });
                                }),
                        );
                    }
                    if let Some(last) = &target.last_run {
                        popup = popup.separator().label(last.text.clone());
                    }
                    popup
                }),
            )
            .into_any_element()
    }
}
