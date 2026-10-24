//! What sits around the code host's areas: signing in to the host, the account menu,
//! picking another repository, and the note while its rate limit runs short.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, Context, Entity, FontWeight, InteractiveElement,
    IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Task, Window,
};
use serde::Deserialize;
use serde_json::json;
use sikemux_wire::codehost::RepoListing;
use sikemux_wire::protocol::CodeHostChoice;

use super::{host_label, HostArea, Said};
use crate::core::Ask;
use crate::icons::Icon;
use crate::palette::{Mark, Row, Spec};
use crate::theme::{Tokens, RADIUS_2};
use crate::ui::{self, Button, Choice, Variant};

/// Who is signed in to the host, as its plugin says.
#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Status {
    pub ok: bool,
    pub message: Option<String>,
    pub account: Option<String>,
    pub host: Option<String>,
    pub login: Option<String>,
    /// Where a token found before anyone signed in came from: `ghCli` or `environment`.
    pub token_source: Option<String>,
    pub token_variable: Option<String>,
    /// The host can be signed in to through the browser.
    pub browser_sign_in: bool,
    pub can_write_workflows: Option<bool>,
    /// GitLab's and Bitbucket's word for what GitHub calls writing workflows.
    pub can_write_ci: Option<bool>,
    pub avatar_url: Option<String>,
}

/// An account signed in to the host.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Listed {
    id: String,
    login: String,
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    is_default: bool,
}

/// How much of the host's rate limit is left.
#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Budget {
    limited: bool,
    resets_at: Option<u64>,
    remaining: Option<u64>,
    limit: Option<u64>,
    near: bool,
}

/// One box of a host's sign-in form.
struct Field {
    name: &'static str,
    label: &'static str,
    placeholder: &'static str,
    secret: bool,
}

/// How a host is signed in to with a token.
struct Form {
    lede: &'static str,
    method: &'static str,
    fields: &'static [Field],
}

const GITHUB: Form = Form {
    lede: "Pull requests, Actions runs and issues for this repository, right beside your changes.",
    method: "signIn",
    fields: &[
        Field {
            name: "host",
            label: "Host",
            placeholder: "github.com",
            secret: false,
        },
        Field {
            name: "token",
            label: "Personal access token",
            placeholder: "ghp_… or github_pat_…",
            secret: true,
        },
    ],
};

const GITLAB: Form = Form {
    lede: "Merge requests, pipelines, issues and your To-Do list for this project, right beside your changes.",
    method: "signInWithToken",
    fields: &[
        Field { name: "host", label: "Server", placeholder: "gitlab.com", secret: false },
        Field { name: "token", label: "Personal access token", placeholder: "glpat-…", secret: true },
    ],
};

const BITBUCKET: Form = Form {
    lede: "Pull requests, pipelines and reviews for this repository, right beside your changes.",
    method: "signInWithToken",
    fields: &[
        Field {
            name: "token",
            label: "Token",
            placeholder: "ATATT… or ATCTT…",
            secret: true,
        },
        Field {
            name: "email",
            label: "Atlassian account email",
            placeholder: "you@example.com",
            secret: false,
        },
    ],
};

/// Whether `typed` names a repository as `owner/name`.
fn is_slug(typed: &str) -> bool {
    let allowed = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    };
    typed
        .split_once('/')
        .is_some_and(|(owner, name)| allowed(owner) && allowed(name))
}

fn form_of(host: &str) -> &'static Form {
    match host {
        "gitlab" => &GITLAB,
        "bitbucket" => &BITBUCKET,
        _ => &GITHUB,
    }
}

/// Where a token for `host` is made, on the server typed in, if any.
fn tokens_page(host: &str, server: &str) -> String {
    let server = server.trim().trim_start_matches("https://");
    let server = server.split('/').next().unwrap_or_default();
    match host {
        "gitlab" => {
            format!(
            "https://{}/-/user_settings/personal_access_tokens?name=Sikemux&scopes=api,read_user",
            if server.is_empty() { "gitlab.com" } else { server }
        )
        }
        "bitbucket" => "https://id.atlassian.com/manage-profile/security/api-tokens".to_owned(),
        _ => format!(
            "https://{}/settings/tokens/new?scopes=repo,workflow&description=Sikemux",
            if server.is_empty() {
                "github.com"
            } else {
                server
            }
        ),
    }
}

/// The sign-in form while it shows.
pub struct SignIn {
    fields: Vec<Entity<InputState>>,
    busy: bool,
    refused: Option<SharedString>,
    /// The browser sign-in on its way, which closing the form stops.
    browser: Option<Task<()>>,
    /// Signing in adds another account rather than the first.
    adding: bool,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

fn clock(epoch_secs: u64) -> String {
    let local = chrono::DateTime::from_timestamp(epoch_secs as i64, 0)
        .map(|at| at.with_timezone(&chrono::Local));
    local
        .map(|at| at.format("%-I:%M %p").to_string())
        .unwrap_or_default()
}

fn wait_in_words(secs: u64) -> String {
    let secs = secs.max(1);
    if secs < 60 {
        return format!("{secs}s");
    }
    let minutes = secs.div_ceil(60);
    if minutes < 60 {
        return format!("{minutes} min");
    }
    format!("{} h {} min", minutes / 60, minutes % 60)
}

/// What to tell the person about the limit, or nothing while there is plenty left.
fn rate_note(host: &str, budget: &Budget, now: u64) -> Option<(bool, String)> {
    if let (true, Some(resets)) = (budget.limited, budget.resets_at) {
        return Some((
            true,
            format!(
                "{host}'s rate limit is used up. Sikemux is holding requests until {}, in {}.",
                clock(resets),
                wait_in_words(resets.saturating_sub(now))
            ),
        ));
    }
    if !budget.near {
        return None;
    }
    Some((
        false,
        match (budget.remaining, budget.limit) {
            (Some(remaining), Some(limit)) => {
                let until = budget
                    .resets_at
                    .map(|resets| format!(", until {}", clock(resets)))
                    .unwrap_or_default();
                format!("{remaining} of {limit} {host} requests left this hour{until}.")
            }
            _ => format!("Close to {host}'s hourly request limit."),
        },
    ))
}

impl HostArea {
    /// Asks the host who is signed in, and how much of its rate limit is left.
    pub(super) fn read_status(&mut self, cx: &mut Context<Self>) {
        let status = self.plugin("status", json!({}));
        cx.spawn(async move |area, cx| {
            let status = status.await;
            area.update(cx, |area, cx| {
                area.status = Some(
                    status
                        .and_then(|value| {
                            serde_json::from_value::<Status>(value)
                                .map_err(|error| error.to_string().into())
                        })
                        .map(|mut status| {
                            let drawn = status.avatar_url.is_none() && area.host.host == "github";
                            if let (true, Some(login)) = (drawn, status.login.clone()) {
                                let host = status.host.as_deref().unwrap_or("github.com");
                                status.avatar_url = Some(if host == "github.com" {
                                    format!("https://avatars.githubusercontent.com/{login}?s=64")
                                } else {
                                    format!("https://{host}/{login}.png?size=64")
                                });
                            }
                            status
                        }),
                );
                if area.signed_in() {
                    area.signing = None;
                    area.read_budget(cx);
                    area.watch();
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn signed_in(&self) -> bool {
        matches!(&self.status, Some(Ok(status)) if status.ok)
    }

    pub(super) fn login(&self) -> Option<&str> {
        match &self.status {
            Some(Ok(status)) => status.login.as_deref().filter(|login| !login.is_empty()),
            _ => None,
        }
    }

    /// Reads the rate limit again, and once more when a spent one resets.
    pub(super) fn read_budget(&self, cx: &mut Context<Self>) {
        let budget = self.plugin("rateLimit", json!({}));
        cx.spawn(async move |area, cx| {
            let budget: Option<Budget> = budget
                .await
                .ok()
                .and_then(|value| serde_json::from_value(value).ok());
            let resets = budget
                .as_ref()
                .filter(|budget| budget.limited)
                .and_then(|budget| budget.resets_at);
            let wait = area
                .update(cx, |area, cx| {
                    area.budget = budget;
                    cx.notify();
                    let now = now_secs();
                    match resets {
                        Some(resets) if resets <= now => Some(area.budget_again.next()),
                        resets => {
                            area.budget_again.reset();
                            resets.map(|resets| Duration::from_secs(resets - now + 1))
                        }
                    }
                })
                .ok()
                .flatten();
            if let Some(wait) = wait {
                cx.background_executor().timer(wait).await;
                area.update(cx, |area, cx| {
                    area.read_budget(cx);
                    area.watch();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Whether the signed-in account may start, re-run, cancel and delete CI runs.
    pub(crate) fn can_write_ci(&self) -> bool {
        self.status
            .as_ref()
            .and_then(|status| status.as_ref().ok())
            .and_then(|status| status.can_write_workflows.or(status.can_write_ci))
            .unwrap_or(false)
    }

    fn start_sign_in(&mut self, adding: bool, window: &mut Window, cx: &mut Context<Self>) {
        let status = self.status.as_ref().and_then(|status| status.as_ref().ok());
        let server = status
            .and_then(|status| status.host.clone())
            .filter(|host| !matches!(host.as_str(), "github.com" | "gitlab.com"))
            .unwrap_or_default();
        let fields = form_of(&self.host.host)
            .fields
            .iter()
            .map(|field| {
                cx.new(|cx| {
                    let mut input = InputState::new(window, cx)
                        .placeholder(field.placeholder)
                        .masked(field.secret);
                    if field.name == "host" && !server.is_empty() {
                        input.set_value(server.clone(), window, cx);
                    }
                    input
                })
            })
            .collect();
        self.signing = Some(SignIn {
            fields,
            busy: false,
            refused: None,
            browser: None,
            adding,
        });
        cx.notify();
    }

    fn sign_in_with_token(&mut self, cx: &mut Context<Self>) {
        let form = form_of(&self.host.host);
        let Some(signing) = &mut self.signing else {
            return;
        };
        let mut params = serde_json::Map::new();
        for (field, input) in form.fields.iter().zip(&signing.fields) {
            let value = input.read(cx).value().trim().to_owned();
            params.insert(
                field.name.to_owned(),
                if value.is_empty() {
                    serde_json::Value::Null
                } else {
                    value.into()
                },
            );
        }
        if params.get("token").is_none_or(serde_json::Value::is_null) {
            return;
        }
        signing.busy = true;
        signing.refused = None;
        let signed = self.plugin(form.method, params.into());
        self.signed(signed, cx);
    }

    /// Signs in with the token the shell or `gh` already holds.
    fn sign_in_borrowed(&mut self, cx: &mut Context<Self>) {
        let host = self
            .status
            .as_ref()
            .and_then(|status| status.as_ref().ok())
            .and_then(|status| status.host.clone())
            .unwrap_or_else(|| "github.com".into());
        if let Some(signing) = &mut self.signing {
            signing.busy = true;
        }
        let signed = self.plugin("signIn", json!({ "host": host }));
        self.signed(signed, cx);
    }

    fn sign_in_with_browser(&mut self, cx: &mut Context<Self>) {
        let call = self.plugin_call("signInWithBrowser", json!({}));
        let mut streaming = self.core.stream("pluginStream", call);
        let waiting = cx.spawn(async move |area, cx| {
            let mut result = None;
            let mut failed = None;
            while let Some(part) = streaming.parts.recv().await {
                for item in part.items {
                    match item["url"].as_str() {
                        Some(url) => {
                            let url = url.to_owned();
                            cx.update(|cx| cx.open_url(&url));
                        }
                        None => result = Some(item),
                    }
                }
                if let Some(error) = part.error {
                    let message = error["message"].as_str().map(str::to_owned);
                    failed = Some(message.unwrap_or_else(|| error.to_string()));
                }
                if part.done {
                    break;
                }
            }
            let signed = match (result, failed) {
                (_, Some(failed)) => Err(failed.into()),
                (Some(result), None) => Ok(result),
                (None, None) => Err("the sign-in ended before it finished".into()),
            };
            area.update(cx, |area, cx| area.took_sign_in(signed, cx))
                .ok();
        });
        let Some(signing) = &mut self.signing else {
            return;
        };
        signing.busy = true;
        signing.refused = None;
        signing.browser = Some(waiting);
        cx.notify();
    }

    fn signed(
        &mut self,
        signed: impl std::future::Future<Output = Result<serde_json::Value, SharedString>> + 'static,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |area, cx| {
            let signed = signed.await;
            area.update(cx, |area, cx| area.took_sign_in(signed, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    fn took_sign_in(
        &mut self,
        signed: Result<serde_json::Value, SharedString>,
        cx: &mut Context<Self>,
    ) {
        let status = signed.and_then(|value| {
            serde_json::from_value::<Status>(value).map_err(|error| error.to_string().into())
        });
        match status {
            Ok(status) if status.ok => {
                let adding = self.signing.as_ref().is_some_and(|signing| signing.adding);
                self.signing = None;
                if adding || self.account.is_some() {
                    self.choose_account(status.account.clone(), cx);
                } else {
                    self.read_status(cx);
                }
            }
            Ok(status) => {
                let host = host_label(&self.host.host);
                self.refuse(
                    status
                        .message
                        .unwrap_or_else(|| format!("{host} turned that token down"))
                        .into(),
                );
            }
            Err(error) => self.refuse(error),
        }
        cx.notify();
    }

    fn refuse(&mut self, refused: SharedString) {
        if let Some(signing) = &mut self.signing {
            signing.busy = false;
            signing.browser = None;
            signing.refused = Some(refused);
        }
    }

    fn sign_out(&mut self, cx: &mut Context<Self>) {
        let login = self.login().unwrap_or("out").to_owned();
        let host = host_label(&self.host.host).to_owned();
        let out = self.plugin("signOut", json!({}));
        cx.spawn(async move |area, cx| {
            let out = out.await;
            area.update(cx, |area, cx| {
                match out {
                    Ok(_) => {
                        area.say(
                            Said::Done(format!("Signed {login} out of {host}").into()),
                            cx,
                        );
                        if area.account.is_some() {
                            area.choose_account(None, cx);
                        }
                        area.read_status(cx);
                    }
                    Err(error) => area.say(
                        Said::Failed(format!("Could not sign out: {error}").into()),
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn make_default(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self
            .status
            .as_ref()
            .and_then(|status| status.as_ref().ok()?.account.clone())
        else {
            return;
        };
        let login = self.login().unwrap_or("this account").to_owned();
        let done = self.plugin("setDefaultAccount", json!({ "id": id }));
        cx.spawn(async move |area, cx| {
            let done = done.await;
            area.update(cx, |area, cx| {
                let said = match done {
                    Ok(_) => Said::Done(format!("New projects open as {login}").into()),
                    Err(error) => Said::Failed(
                        format!("Could not change the default account: {error}").into(),
                    ),
                };
                area.say(said, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Opens the account menu, reading the host's accounts for it.
    fn open_menu(&mut self, cx: &mut Context<Self>) {
        self.menu = true;
        let listed = self.plugin("accounts", json!({}));
        cx.spawn(async move |area, cx| {
            let listed: Vec<Listed> = listed
                .await
                .ok()
                .and_then(|value| serde_json::from_value(value).ok())
                .unwrap_or_default();
            area.update(cx, |area, cx| {
                area.accounts = listed;
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Keeps the project on `account` on its host, or on whichever its remote finds.
    fn choose_account(&mut self, account: Option<String>, cx: &mut Context<Self>) {
        self.account = account.clone();
        self.pick_code_host(move |choice| choice.account = account, cx);
        self.read_status(cx);
    }

    /// Changes what the project picked on its host, which the core keeps.
    fn pick_code_host(&self, change: impl FnOnce(&mut CodeHostChoice), cx: &mut Context<Self>) {
        let (folder, host) = (self.project.clone(), self.host.host.clone());
        self.bench
            .update(cx, |bench, _| {
                let Some(project) = bench
                    .projects()
                    .iter()
                    .find(|project| project.path.to_string_lossy() == folder)
                else {
                    return;
                };
                let mut hosts = project.code_hosts.clone();
                let mut choice = hosts.remove(&host).unwrap_or_default();
                change(&mut choice);
                if choice.repo.is_some() || choice.account.is_some() {
                    hosts.insert(host, choice);
                }
                let params = json!({ "id": project.id, "patch": { "codeHosts": hosts } });
                bench.target().call("updateProject", params);
            })
            .ok();
    }

    /// The repositories pinned on the host, kept with its plugin's settings.
    fn pinned(&self, cx: &App) -> Vec<String> {
        let Some(bench) = self.bench.upgrade() else {
            return Vec::new();
        };
        let settings = bench.read(cx).preferences().plugin_settings;
        settings
            .get(&self.host.host)
            .and_then(|kept| kept["pinned"].as_array())
            .into_iter()
            .flatten()
            .filter_map(|slug| {
                slug.as_str()
                    .filter(|slug| is_slug(slug))
                    .map(str::to_owned)
            })
            .collect()
    }

    /// Keeps the repository shown here at the top of the picker, or stops keeping it.
    fn toggle_pin(&mut self, cx: &mut Context<Self>) {
        let Some(bench) = self.bench.upgrade() else {
            return;
        };
        let mut all = bench.read(cx).preferences().plugin_settings;
        let mut pinned = self.pinned(cx);
        match pinned.iter().position(|slug| *slug == self.host.repo) {
            Some(at) => {
                pinned.remove(at);
            }
            None => pinned.push(self.host.repo.clone()),
        }
        let mut kept = all.remove(&self.host.host).unwrap_or_else(|| json!({}));
        if let Some(kept) = kept.as_object_mut() {
            kept.insert("pinned".into(), json!(pinned));
        }
        all.insert(self.host.host.clone(), kept);
        let params = json!({ "key": "pluginSettings", "value": all });
        let _ = self.core.asks().send(Ask::Call {
            method: "setPreference",
            params,
        });
    }

    /// Opens the palette of the person's repositories on the host, to show another here.
    pub(crate) fn pick_repo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = false;
        cx.notify();
        let mine = self.plugin("myRepos", json!({ "limit": 50 }));
        let area = cx.entity().downgrade();
        let current = self.host.repo.clone();
        let pinned = self.pinned(cx);
        let row = {
            let current = current.clone();
            move |slug: String, group: &'static str, about: String| {
                let meta = if slug == current {
                    "shown here".to_owned()
                } else {
                    about
                };
                Row::new(slug.clone(), slug)
                    .meta(meta)
                    .mark(Mark::Icon(Icon::Git))
                    .group(group)
            }
        };
        let kept: Vec<Row> = pinned
            .iter()
            .map(|slug| row(slug.clone(), "Pinned", "pinned".into()))
            .collect();
        let mut first = kept.clone();
        first.push(Row::new("", "Reading your repositories…").disabled(true));
        let spec = Spec::new(
            "codehost.repo",
            "search your repositories, or type owner/repo",
            first,
            move |row, _, _, cx| {
                let slug = row.key.to_string();
                if slug.is_empty() {
                    return;
                }
                area.update(cx, |area, cx| {
                    area.pick_code_host(move |choice| choice.repo = Some(slug), cx);
                })
                .ok();
            },
        )
        .typed(|typed| {
            is_slug(typed).then(|| {
                Row::new(typed.to_owned(), typed.to_owned())
                    .meta("open it")
                    .mark(Mark::Icon(Icon::Git))
            })
        })
        .empty("no repositories match");
        self.bench
            .update(cx, |bench, cx| bench.toggle_palette(spec, window, cx))
            .ok();
        let bench = self.bench.clone();
        cx.spawn(async move |_, cx| {
            let mine: Result<Vec<RepoListing>, SharedString> = mine.await.and_then(|value| {
                serde_json::from_value(value).map_err(|error| error.to_string().into())
            });
            let mut rows = kept;
            match mine {
                Ok(mine) => rows.extend(
                    mine.into_iter()
                        .filter(|repo| !pinned.contains(&repo.slug))
                        .map(|repo| {
                            let mut about: Vec<String> = Vec::new();
                            if repo.private {
                                about.push("private".into());
                            }
                            if repo.archived {
                                about.push("archived".into());
                            }
                            if let Some(pushed) = repo.pushed_at.as_deref() {
                                about.push(super::ago(pushed));
                            }
                            row(repo.slug, "Your repositories", about.join(" · "))
                        }),
                ),
                Err(error) => rows.push(
                    Row::new("", format!("Could not read your repositories: {error}"))
                        .disabled(true),
                ),
            }
            bench
                .update(cx, |bench, cx| {
                    if let Some(palette) = bench
                        .palette
                        .as_mut()
                        .filter(|open| open.id() == "codehost.repo")
                    {
                        palette.refill(rows, cx);
                    }
                    bench.parts.redraw(&bench.parts.finder, cx);
                })
                .ok();
        })
        .detach();
    }

    /// The menu of who is signed in, to switch accounts or repositories.
    pub(crate) fn account_menu(&mut self, cx: &mut Context<Self>) -> Option<ui::Choices> {
        if !self.menu {
            return None;
        }
        let status = self.status.as_ref()?.as_ref().ok()?.clone();
        let login = status.login.clone().unwrap_or_default();
        let current = status.account.clone();
        let mut choices: Vec<Choice> = Vec::new();
        let area = cx.entity().downgrade();
        if self.accounts.len() > 1 {
            for (index, listed) in self.accounts.iter().enumerate() {
                let (id, area) = (listed.id.clone(), area.clone());
                let mut words = listed.login.clone();
                if let Some(host) = listed
                    .host
                    .as_deref()
                    .filter(|host| !matches!(*host, "github.com" | "gitlab.com" | "bitbucket.org"))
                {
                    words.push_str(&format!(" · {host}"));
                }
                if Some(&listed.id) == current.as_ref() {
                    words.push_str(" ✓");
                }
                let key = char::from_digit(index as u32 + 1, 10);
                choices.push(Choice::new(key, words, move |_, cx| {
                    let id = id.clone();
                    area.update(cx, |area, cx| {
                        area.menu = false;
                        area.choose_account(Some(id), cx);
                        cx.notify();
                    })
                    .ok();
                }));
            }
        }
        let adding = area.clone();
        choices.push(Choice::new(
            'a',
            "Add another account…",
            move |window, cx| {
                adding
                    .update(cx, |area, cx| {
                        area.menu = false;
                        area.start_sign_in(true, window, cx);
                    })
                    .ok();
            },
        ));
        let picking = area.clone();
        choices.push(Choice::new(
            'r',
            "Choose another repository…",
            move |window, cx| {
                picking
                    .update(cx, |area, cx| area.pick_repo(window, cx))
                    .ok();
            },
        ));
        let default = self
            .accounts
            .iter()
            .find(|listed| Some(&listed.id) == current.as_ref())
            .is_none_or(|listed| listed.is_default);
        if self.accounts.len() > 1 && !default {
            let defaulting = area.clone();
            choices.push(Choice::new(
                'd',
                format!("Open new projects as {login}"),
                move |_, cx| {
                    defaulting
                        .update(cx, |area, cx| {
                            area.menu = false;
                            area.make_default(cx);
                            cx.notify();
                        })
                        .ok();
                },
            ));
        }
        let pinned = self.pinned(cx).contains(&self.host.repo);
        let pinning = area.clone();
        choices.push(Choice::new(
            'p',
            if pinned {
                "Unpin this repository"
            } else {
                "Pin this repository"
            },
            move |_, cx| {
                pinning
                    .update(cx, |area, cx| {
                        area.menu = false;
                        area.toggle_pin(cx);
                        cx.notify();
                    })
                    .ok();
            },
        ));
        let leaving = area.clone();
        choices.push(
            Choice::new('o', format!("Sign {login} out"), move |_, cx| {
                leaving
                    .update(cx, |area, cx| {
                        area.menu = false;
                        area.sign_out(cx);
                        cx.notify();
                    })
                    .ok();
            })
            .destructive(),
        );
        let mut title = format!(
            "{login} on {}",
            status
                .host
                .as_deref()
                .unwrap_or(host_label(&self.host.host))
        );
        title.push_str(&format!(" · {}", self.host.repo));
        if status.can_write_workflows == Some(false) {
            title.push_str(
                " · cannot start or re-run workflows: the token lacks the workflow scope",
            );
        }
        let closing = area;
        Some(ui::Choices::new(title, choices).on_no(move |_, cx| {
            closing
                .update(cx, |area, cx| {
                    area.menu = false;
                    cx.notify();
                })
                .ok();
        }))
    }
}

/// Who is signed in, at the end of a list's head, opening the account menu.
pub fn account(area: &HostArea, cx: &mut Context<HostArea>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let host = host_label(&area.host.host);
    let Some(login) = area.login().map(str::to_owned) else {
        return sign_in_chip(area, cx);
    };
    div()
        .id("host-account")
        .flex_none()
        .cursor_pointer()
        .rounded_full()
        .hover(move |chip| chip.bg(t.surface_hover))
        .tooltip(ui::tooltip(format!(
            "{login} on {host} · {}",
            area.host.repo
        )))
        .on_click(cx.listener(|area, _, _, cx| area.open_menu(cx)))
        .child(area.face(Some(&login), area.avatar(), 20., cx))
        .into_any_element()
}

/// Sign in, split as the Run button is: the host's logo in a cell of its own on the plain
/// ground, a hairline, then the words. It opens Pull requests, which asks for the sign-in.
fn sign_in_chip(area: &HostArea, cx: &mut Context<HostArea>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let host = host_label(&area.host.host).to_owned();
    let logo = match area.host.host.as_str() {
        "github" => Icon::Github,
        _ => Icon::Git,
    };
    let bench = area.bench.clone();
    div()
        .id("host-sign-in")
        .flex_none()
        .h(px(28.))
        .flex()
        .rounded(RADIUS_2)
        .overflow_hidden()
        .border_1()
        .border_color(t.border)
        .bg(t.surface)
        .cursor_pointer()
        .text_size(crate::theme::ui_text(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(t.text_secondary)
        .hover(move |chip| chip.bg(t.surface_hover).text_color(t.text_primary))
        .tooltip(ui::tooltip(format!("Sign in to {host}")))
        .on_click(move |_, _, cx| {
            let _ = bench.update(cx, |bench, cx| bench.open_area(super::Area::Pulls, cx));
        })
        .child(
            div()
                .px(px(8.))
                .flex()
                .items_center()
                .border_r_1()
                .border_color(t.border)
                .child(ui::picture(logo, 12., t.text_secondary)),
        )
        .child(div().px(px(10.)).flex().items_center().child("Sign in"))
        .into_any_element()
}

/// The note while the host's rate limit is spent or nearly so.
pub fn budget_note(area: &HostArea, cx: &mut Context<HostArea>) -> Option<AnyElement> {
    let t = Tokens::get(cx).clone();
    let (spent, words) = rate_note(
        host_label(&area.host.host),
        area.budget.as_ref()?,
        now_secs(),
    )?;
    let ink = if spent { t.danger } else { t.warn };
    Some(
        div()
            .mx(px(12.))
            .mt(px(10.))
            .px(px(12.))
            .py(px(8.))
            .rounded(RADIUS_2)
            .border_1()
            .border_color(ink.opacity(0.4))
            .bg(ink.opacity(0.08))
            .text_size(crate::theme::ui_text(12.))
            .text_color(t.text_primary)
            .child(words)
            .into_any_element(),
    )
}

/// Asking the person to sign in to the host, in place of its areas.
pub fn sign_in(area: &mut HostArea, window: &mut Window, cx: &mut Context<HostArea>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let host = host_label(&area.host.host).to_owned();
    match &area.status {
        None => return super::notice(format!("Connecting to {host}"), None, cx),
        Some(Err(error)) => {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .child(super::notice(
                    format!("Could not reach {host}"),
                    Some(error.clone()),
                    cx,
                ))
                .child(div().flex().justify_center().child(
                    Button::new("host-retry", "Try again", Variant::Outline).on_click(cx.listener(
                        |area, _, _, cx| {
                            area.status = None;
                            area.read_status(cx);
                            cx.notify();
                        },
                    )),
                ))
                .into_any_element();
        }
        Some(Ok(_)) => {}
    }
    if area.signing.is_none() {
        area.start_sign_in(false, window, cx);
    }
    let status: Status = area.status.clone().and_then(Result::ok).unwrap_or_default();
    let form = form_of(&area.host.host);
    let Some(signing) = &area.signing else {
        return div().into_any_element();
    };
    let busy = signing.busy;
    let server = form
        .fields
        .iter()
        .zip(&signing.fields)
        .find(|(field, _)| field.name == "host")
        .map(|(_, input)| input.read(cx).value().to_string())
        .unwrap_or_default();
    let page = tokens_page(&area.host.host, &server);
    let borrowed = match status.token_source.as_deref() {
        Some("ghCli") => Some((
            "Continue with the gh CLI".to_owned(),
            "The gh CLI is signed in here, so Sikemux uses its token.".to_owned(),
        )),
        Some("environment") => {
            let variable = status
                .token_variable
                .clone()
                .unwrap_or_else(|| "that token".into());
            Some((
                format!("Continue with {variable}"),
                format!("{variable} is set in your shell, so Sikemux uses that token."),
            ))
        }
        _ => None,
    }
    .filter(|_| area.host.host == "github" && !signing.adding);
    let mut column = div()
        .w(px(380.))
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(ui::icon(Icon::Git, 18.).text_color(t.text_primary))
                .child(
                    ui::text(
                        if signing.adding {
                            format!("Add another {host} account")
                        } else {
                            format!("Sign in to {host}")
                        },
                        15.,
                        FontWeight::SEMIBOLD,
                    )
                    .text_color(t.text_primary),
                ),
        )
        .child(ui::meta(form.lede, cx));
    if let Some((label, note)) = borrowed {
        column = column.child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    Button::new("host-borrowed", label, Variant::Primary)
                        .disabled(busy)
                        .on_click(cx.listener(|area, _, _, cx| area.sign_in_borrowed(cx))),
                )
                .child(ui::meta(note, cx)),
        );
    }
    if status.browser_sign_in {
        column = column.child(
            Button::new(
                "host-browser",
                if signing.browser.is_some() {
                    "Waiting for the browser…".to_owned()
                } else {
                    format!("Continue with {host}")
                },
                Variant::Primary,
            )
            .disabled(busy)
            .on_click(cx.listener(|area, _, _, cx| area.sign_in_with_browser(cx))),
        );
    }
    for (index, (field, input)) in form.fields.iter().zip(&signing.fields).enumerate() {
        column = column.child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(ui::meta(field.label, cx))
                .child(
                    div()
                        .id(("host-field", index))
                        .px(px(10.))
                        .py(px(6.))
                        .rounded(RADIUS_2)
                        .border_1()
                        .border_color(t.border)
                        .bg(t.surface)
                        .text_size(crate::theme::ui_text(12.5))
                        .child(Input::new(input).appearance(false)),
                ),
        );
    }
    if let Some(refused) = &signing.refused {
        column = column.child(ui::meta(refused.clone(), cx).text_color(t.danger));
    }
    if let Some(message) = status
        .message
        .as_ref()
        .filter(|_| signing.refused.is_none())
    {
        column = column.child(ui::meta(message.clone(), cx));
    }
    column = column.child(
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                Button::new("host-token-page", "Create a token", Variant::Outline)
                    .icon(Icon::Globe)
                    .on_click(move |_, _, cx| cx.open_url(&page)),
            )
            .child(div().flex_1())
            .when(signing.adding, |row| {
                row.child(
                    Button::new("host-sign-in-cancel", "Cancel", Variant::Outline).on_click(
                        cx.listener(|area, _, _, cx| {
                            area.signing = None;
                            cx.notify();
                        }),
                    ),
                )
            })
            .child(
                Button::new(
                    "host-sign-in",
                    if busy {
                        "Signing in…"
                    } else {
                        "Sign in with token"
                    },
                    Variant::Primary,
                )
                .disabled(busy)
                .on_click(cx.listener(|area, _, _, cx| area.sign_in_with_token(cx))),
            ),
    );
    div()
        .id("host-sign-in-page")
        .size_full()
        .overflow_y_scroll()
        .flex()
        .justify_center()
        .pt(px(48.))
        .child(column)
        .into_any_element()
}

/// Whether the status answered says to sign in before anything else shows.
pub fn needs_sign_in(area: &HostArea) -> bool {
    !area.signed_in() || area.signing.as_ref().is_some_and(|signing| signing.adding)
}
