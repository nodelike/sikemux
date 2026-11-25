//! What a lead can start workers on here: every signed-in agent, whether it runs on a plan or
//! per token, each plan's remaining room from the same readings LIMITS shows, every exact model
//! id `agent_spawn` takes, and the connected model providers.

use std::path::Path;

use serde_json::{json, Value};
use sikemux_calls::agent_accounts::READING_STALE_AFTER_MS;
use sikemux_process::clock::epoch_ms;
use sikemux_wire::protocol::ChatContext;

use crate::agent_accounts::status::{AgentStatus, FREE_MODELS};
use crate::agent_accounts::usage::{self, Reading, UsageWindow};
use crate::launch::providers::Kind as ProviderKind;
use crate::server::activity_log::Prices;
use crate::server::workspace::Launcher;
use crate::server::workspace_state::WorkspaceParts;

use super::Core;

const INLINE_MODELS: usize = 12;
const PREVIEW_MODELS: usize = 4;

struct Catalog {
    agents: Vec<StartableAgent>,
    providers: Vec<StartableProvider>,
}

#[derive(Clone)]
struct StartableAgent {
    cli: String,
    billing: Billing,
    room: Option<String>,
    room_levels: Vec<Vec<Option<u32>>>,
    models: Vec<String>,
}

pub(crate) struct AgentRoom {
    pub(crate) cli: String,
    pub(crate) summary: String,
    pub(crate) accounts: Vec<Vec<Option<u32>>>,
}

pub(crate) struct ChiefCatalog {
    pub(crate) static_contexts: Vec<ChatContext>,
    pub(crate) rooms: Vec<AgentRoom>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Billing {
    Plan,
    PlanAndPerToken,
    PerToken,
    Free,
}

impl Billing {
    fn label(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::PlanAndPerToken => "plan + paid per token",
            Self::PerToken => "paid per token",
            Self::Free => "free models",
        }
    }

    fn priced_models(self) -> bool {
        matches!(self, Self::PerToken | Self::PlanAndPerToken)
    }
}

struct StartableProvider {
    id: String,
    kind: ProviderKind,
    agents: Vec<String>,
    models: Vec<String>,
}

impl StartableProvider {
    fn billing(&self) -> &'static str {
        match self.kind {
            ProviderKind::Api => "paid per token",
            ProviderKind::Plan => "plan",
            ProviderKind::Local => "local, free",
        }
    }
}

pub(crate) fn for_chief(core: &Core, project: &Path) -> ChiefCatalog {
    let catalog = collect(core, project, epoch_ms());
    let providers = providers_text(&catalog.providers).map(|text| {
        crate::sessions::for_agent::sikemux_context(
            "model-providers",
            "Model providers you can pay workers with",
            &text,
        )
    });
    let agents = agents_text_with_room(&catalog.agents, false).map(|text| {
        crate::sessions::for_agent::sikemux_context(
            "can-start",
            "What you can start workers on",
            &text,
        )
    });
    let rooms = catalog
        .agents
        .into_iter()
        .filter_map(|agent| {
            let summary = agent.room?;
            Some(AgentRoom {
                cli: agent.cli,
                summary,
                accounts: agent.room_levels,
            })
        })
        .collect();
    ChiefCatalog {
        static_contexts: providers.into_iter().chain(agents).collect(),
        rooms,
    }
}

pub(crate) fn quota_changed(
    previous: Option<&[Vec<Option<u32>>]>,
    current: &[Vec<Option<u32>>],
) -> bool {
    const SHIFT_THRESHOLD: u32 = 15;
    let Some(previous) = previous else {
        return true;
    };
    if previous.len() != current.len() {
        return true;
    }
    previous
        .iter()
        .zip(current.iter())
        .any(|(prev_acct, cur_acct)| {
            prev_acct.len() != cur_acct.len()
                || prev_acct
                    .iter()
                    .zip(cur_acct.iter())
                    .any(|(a, b)| match (a, b) {
                        (None, None) => false,
                        (Some(prev), Some(now)) => {
                            quota_band(*prev) != quota_band(*now)
                                || prev.abs_diff(*now) >= SHIFT_THRESHOLD
                        }
                        _ => true,
                    })
        })
}

fn quota_band(left: u32) -> u8 {
    match left {
        0 => 0,
        1..10 => 1,
        10..25 => 2,
        _ => 3,
    }
}

pub(crate) fn room_context(rooms: &[&AgentRoom]) -> Option<ChatContext> {
    if rooms.is_empty() {
        return None;
    }
    let text = rooms
        .iter()
        .map(|room| format!("{}: {}", room.cli, room.summary))
        .collect::<Vec<_>>()
        .join(" | ");
    Some(crate::sessions::for_agent::sikemux_context(
        "quota-room",
        "Plan room",
        &text,
    ))
}

pub(crate) fn for_tool(core: &Core, project: &Path, filter: &str) -> Result<Value, String> {
    let catalog = collect(core, project, epoch_ms());
    let wanted = filter.trim().to_ascii_lowercase();
    let note = "Pass the exact model id (before any $price or label) to agent_spawn; per-token prices are USD/M in/out.";
    if wanted == "all" {
        let mut fields = serde_json::Map::new();
        fields.insert("note".into(), note.into());
        fields.insert(
            "agents".into(),
            catalog
                .agents
                .iter()
                .map(agent_json)
                .collect::<Vec<_>>()
                .into(),
        );
        if !catalog.providers.is_empty() {
            fields.insert(
                "providers".into(),
                catalog
                    .providers
                    .iter()
                    .map(provider_json)
                    .collect::<Vec<_>>()
                    .into(),
            );
        }
        return Ok(Value::Object(fields));
    }
    if let Some(agent) = catalog.agents.iter().find(|agent| agent.cli == wanted) {
        let mut answer = agent_json(agent);
        if let Some(fields) = answer.as_object_mut() {
            fields.insert("note".into(), note.into());
        }
        return Ok(answer);
    }
    if let Some(provider) = catalog
        .providers
        .iter()
        .find(|provider| provider.id.eq_ignore_ascii_case(&wanted))
    {
        let mut answer = provider_json(provider);
        if let Some(fields) = answer.as_object_mut() {
            fields.insert("note".into(), note.into());
        }
        return Ok(answer);
    }
    let mut available: Vec<&str> = catalog
        .agents
        .iter()
        .map(|agent| agent.cli.as_str())
        .collect();
    available.extend(
        catalog
            .providers
            .iter()
            .map(|provider| provider.id.as_str()),
    );
    match available.is_empty() {
        true => Err("No signed-in agents are ready on this host".into()),
        false => Err(format!(
            "{wanted} is not signed in here. Available: {}, or \"all\"",
            available.join(", ")
        )),
    }
}

fn agent_json(agent: &StartableAgent) -> Value {
    let mut fields = serde_json::Map::new();
    fields.insert("cli".into(), agent.cli.clone().into());
    fields.insert("billing".into(), agent.billing.label().into());
    if let Some(room) = &agent.room {
        fields.insert("room".into(), room.clone().into());
    }
    fields.insert("models".into(), agent.models.clone().into());
    Value::Object(fields)
}

fn provider_json(provider: &StartableProvider) -> Value {
    json!({
        "provider": provider.id,
        "billing": provider.billing(),
        "agents": provider.agents,
        "models": provider.models,
    })
}

fn collect(core: &Core, project: &Path, now: u64) -> Catalog {
    let mut grouped: Vec<(String, Vec<Launcher>)> = Vec::new();
    for launcher in core.workspaces().chat_launchers() {
        if !is_signed_in(&launcher) {
            continue;
        }
        let kind = launcher.agent.provider.clone();
        match grouped.iter_mut().find(|(seen, _)| *seen == kind) {
            Some((_, list)) => list.push(launcher),
            None => grouped.push((kind, vec![launcher])),
        }
    }
    let kinds: Vec<String> = grouped.iter().map(|(kind, _)| kind.clone()).collect();
    let prices = core.activity_log().prices();
    let priced = |model: &str| match prices.per_million(model) {
        Some((input, output)) => format!("{model} ${}/${}", dollars(input), dollars(output)),
        None => model.to_owned(),
    };
    let providers: Vec<StartableProvider> = super::model_providers::on_hand(core, project)
        .into_iter()
        .filter(|provider| provider.agents.iter().any(|agent| kinds.contains(agent)))
        .map(|provider| {
            let models = provider
                .models
                .iter()
                .map(|model| match provider.kind {
                    ProviderKind::Api => priced(model),
                    ProviderKind::Plan | ProviderKind::Local => model.clone(),
                })
                .collect();
            let agents = provider
                .agents
                .into_iter()
                .filter(|agent| kinds.contains(agent))
                .collect();
            StartableProvider {
                id: provider.id,
                kind: provider.kind,
                agents,
                models,
            }
        })
        .collect();
    let agents = grouped
        .into_iter()
        .map(|(cli, launchers)| {
            let (billing, room, room_levels) = billing_and_room(&cli, &launchers, now);
            let models = offered_models(core, &cli, billing.priced_models(), &prices);
            StartableAgent {
                cli,
                billing,
                room,
                room_levels,
                models,
            }
        })
        .collect();
    Catalog { agents, providers }
}

fn is_signed_in(launcher: &Launcher) -> bool {
    match &launcher.status {
        Some(AgentStatus::Ready { .. }) => true,
        None | Some(AgentStatus::Unknown) => usage::last_reading(
            &launcher.agent.provider,
            launcher.agent.config_path.as_deref(),
        )
        .is_some(),
        Some(AgentStatus::Missing | AgentStatus::Broken { .. } | AgentStatus::SignedOut) => false,
    }
}

fn billing_and_room(
    cli: &str,
    launchers: &[Launcher],
    now: u64,
) -> (Billing, Option<String>, Vec<Vec<Option<u32>>>) {
    let mut plan_readings: Vec<Option<Reading>> = Vec::new();
    let mut any_free = false;
    let mut any_per_token = false;
    for launcher in launchers {
        let account_kind = match &launcher.status {
            Some(AgentStatus::Ready { account }) => account.as_deref(),
            _ => None,
        };
        let reading = usage::last_reading(cli, launcher.agent.config_path.as_deref());
        let has_limits = reading
            .as_ref()
            .is_some_and(|read| !read.usage.windows.is_empty());
        let is_api_key = account_kind == Some("apiKey");
        let is_free = account_kind == Some(FREE_MODELS);
        let is_sub = account_kind == Some("subscription");

        if cli == "opencode" {
            if has_limits {
                plan_readings.push(reading);
                any_per_token = true;
            } else if is_free {
                any_free = true;
            } else {
                any_per_token = true;
            }
        } else if !is_api_key && (is_sub || has_limits) {
            plan_readings.push(reading);
        } else if is_free {
            any_free = true;
        } else {
            any_per_token = true;
        }
    }
    let billing = match (!plan_readings.is_empty(), any_per_token, any_free) {
        (true, true, _) => Billing::PlanAndPerToken,
        (true, false, _) => Billing::Plan,
        (false, false, true) => Billing::Free,
        (false, _, _) => Billing::PerToken,
    };
    let room = (!plan_readings.is_empty()).then(|| format_accounts_room(&plan_readings, now));
    let room_levels = plan_readings
        .iter()
        .map(|reading| reading_levels(reading.as_ref(), now))
        .collect();
    (billing, room, room_levels)
}

fn reading_levels(reading: Option<&Reading>, now: u64) -> Vec<Option<u32>> {
    let Some(reading) = reading else {
        return vec![None];
    };
    let windows = ordered_windows(&reading.usage.windows);
    if windows.is_empty() {
        return vec![None];
    }
    windows
        .into_iter()
        .map(|window| {
            let left = (100.0 - window.used_at(now)).clamp(0.0, 100.0).round() as u32;
            Some(left)
        })
        .collect()
}

fn format_accounts_room(readings: &[Option<Reading>], now: u64) -> String {
    if let [only] = readings {
        return format_reading(only.as_ref(), now);
    }
    readings
        .iter()
        .enumerate()
        .map(|(index, reading)| {
            format!(
                "acct {}: {}",
                index + 1,
                format_reading(reading.as_ref(), now)
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn format_reading(reading: Option<&Reading>, now: u64) -> String {
    let Some(reading) = reading else {
        return "room unread".to_owned();
    };
    let tier = reading
        .usage
        .tier
        .as_deref()
        .or(reading.usage.plan.as_deref())
        .map(str::trim)
        .filter(|tier| !tier.is_empty());
    let windows = ordered_windows(&reading.usage.windows);
    let mut parts: Vec<String> = windows
        .into_iter()
        .map(|window| format_window(window, now))
        .collect();
    if parts.is_empty() {
        parts.push("room unread".to_owned());
    }
    let mut text = match tier {
        Some(tier) => format!("{tier}: {}", parts.join(", ")),
        None => parts.join(", "),
    };
    if now.saturating_sub(reading.read_at) >= READING_STALE_AFTER_MS {
        text.push_str(" (stale)");
    }
    text
}

fn ordered_windows(windows: &[UsageWindow]) -> Vec<&UsageWindow> {
    let mut plan: Vec<&UsageWindow> = windows
        .iter()
        .filter(|window| window.scope.is_none())
        .collect();
    plan.sort_by_key(|window| window.window_minutes.unwrap_or(u64::MAX));

    let mut scoped_groups: Vec<(&str, Vec<&UsageWindow>)> = Vec::new();
    for window in windows.iter().filter(|window| window.scope.is_some()) {
        let scope = window.scope.as_deref().unwrap_or_default();
        match scoped_groups.iter_mut().find(|(seen, _)| *seen == scope) {
            Some((_, group)) => group.push(window),
            None => scoped_groups.push((scope, vec![window])),
        }
    }
    for (_, group) in &mut scoped_groups {
        group.sort_by_key(|window| window.window_minutes.unwrap_or(u64::MAX));
    }
    plan.into_iter()
        .chain(scoped_groups.into_iter().flat_map(|(_, group)| group))
        .collect()
}

fn format_window(window: &UsageWindow, now: u64) -> String {
    let left = (100.0 - window.used_at(now)).clamp(0.0, 100.0).round() as u32;
    let mut text = format!("{} {left}%", window.label);
    if left < 15 && !window.is_past_reset(now) {
        if let Some(at) = window.resets_at.as_ref().and_then(|at| at.unix_ms()) {
            let hours = at.saturating_sub(now) / 3_600_000;
            let when = match hours {
                0 => "<1h".to_owned(),
                1..=47 => format!("{hours}h"),
                _ => format!("{}d", hours / 24),
            };
            text.push_str(&format!(" (resets {when})"));
        }
    }
    text
}

#[cfg(test)]
fn agents_text(agents: &[StartableAgent]) -> Option<String> {
    agents_text_with_room(agents, true)
}

fn agents_text_with_room(agents: &[StartableAgent], include_room: bool) -> Option<String> {
    if agents.is_empty() {
        return None;
    }
    let any_folded = agents
        .iter()
        .any(|agent| agent.models.len() > INLINE_MODELS);
    let any_priced = agents.iter().any(|agent| agent.billing.priced_models());
    let mut hints = vec!["pass exact model ids to agent_spawn"];
    if any_folded {
        hints.push("call agent_list with models: \"<cli>\" or \"all\" for every model id");
    }
    if any_priced {
        hints.push("prices USD/M in/out");
    }
    let mut lines = vec![format!(
        "Agents you can start workers on here ({}):",
        hints.join("; ")
    )];
    for agent in agents {
        let meta = match (&agent.room, include_room) {
            (Some(room), true) => format!("{}; left: {room}", agent.billing.label()),
            _ => agent.billing.label().to_owned(),
        };
        let models = format_inline_models(&agent.models);
        lines.push(format!("- {} ({meta}): {models}", agent.cli));
    }
    Some(lines.join("\n"))
}

fn providers_text(providers: &[StartableProvider]) -> Option<String> {
    if providers.is_empty() {
        return None;
    }
    let any_folded = providers
        .iter()
        .any(|provider| provider.models.len() > INLINE_MODELS);
    let mut hints = vec!["use agent_spawn's cli, provider and model"];
    if any_folded {
        hints.push("call agent_list with models: \"<provider>\" or \"all\" for every model id");
    }
    hints.push("prices USD/M in/out");
    let mut lines = vec![format!(
        "Model providers connected here ({}):",
        hints.join("; ")
    )];
    for provider in providers {
        let models = format_inline_models(&provider.models);
        lines.push(format!(
            "- {} (for {}; {}): {models}",
            provider.id,
            provider.agents.join(", "),
            provider.billing()
        ));
    }
    Some(lines.join("\n"))
}

fn format_inline_models(models: &[String]) -> String {
    if models.is_empty() {
        return "models not loaded yet (omit model for default)".to_owned();
    }
    if models.len() <= INLINE_MODELS {
        return models.join(", ");
    }
    let mut shown: Vec<String> = models.iter().take(PREVIEW_MODELS).cloned().collect();
    shown.push(format!("+{} more", models.len() - PREVIEW_MODELS));
    shown.join(", ")
}

fn offered_models(core: &Core, kind: &str, priced: bool, prices: &Prices) -> Vec<String> {
    let mut choices: Vec<_> = super::chat::models_of(core, kind)
        .into_iter()
        .flat_map(|picker| picker.choices)
        .filter(|choice| {
            !choice.superseded && !crate::launch::providers::routed_model(&choice.value)
        })
        .map(|choice| {
            let price = priced.then(|| prices.per_million(&choice.value)).flatten();
            (choice, price)
        })
        .collect();
    if priced {
        let recent = core.workspaces().recent_models(kind);
        let recency = |value: &str| {
            recent
                .iter()
                .position(|used| used.model == value)
                .unwrap_or(recent.len())
        };
        choices.sort_by(|(a, a_price), (b, b_price)| {
            let cost = |price: &Option<(f64, f64)>| price.map(|(input, output)| input + output);
            recency(&a.value)
                .cmp(&recency(&b.value))
                .then_with(|| b.is_default.cmp(&a.is_default))
                .then_with(|| match (cost(a_price), cost(b_price)) {
                    (Some(a), Some(b)) => a.total_cmp(&b),
                    (a, b) => b.is_some().cmp(&a.is_some()),
                })
        });
    }
    choices
        .into_iter()
        .map(|(choice, price)| {
            let priced = match price {
                Some((input, output)) => {
                    format!("{} ${}/${}", choice.value, dollars(input), dollars(output))
                }
                None => choice.value.clone(),
            };
            match same_name(&choice.value, &choice.label) {
                true => priced,
                false => format!("{priced} ({})", choice.label),
            }
        })
        .collect()
}

fn same_name(id: &str, label: &str) -> bool {
    fn norm(text: &str) -> impl Iterator<Item = char> + '_ {
        text.chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_lowercase())
    }
    norm(id).eq(norm(label))
}

fn dollars(per_million: f64) -> String {
    let cents = (per_million * 100.0).round() / 100.0;
    format!("{cents}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_accounts::usage::{Usage, UsageResetAt};

    #[test]
    fn every_signed_in_agent_stays_visible_and_long_lists_fold_to_agent_list() {
        let agents = vec![
            StartableAgent {
                cli: "claude".into(),
                billing: Billing::Plan,
                room: Some("acct 1: Ultra: 7d 0% (resets 3h); acct 2: Ultra: 7d 20%".into()),
                room_levels: vec![vec![Some(0)], vec![Some(20)]],
                models: vec![
                    "default (Opus 5.5)".into(),
                    "fable (Fable 5.2)".into(),
                    "sonnet (Sonnet 5.5)".into(),
                    "haiku (Haiku 5.5)".into(),
                ],
            },
            StartableAgent {
                cli: "codex".into(),
                billing: Billing::Plan,
                room: Some("Free: 30d 100%".into()),
                room_levels: vec![vec![Some(100)]],
                models: (0..29).map(|i| format!("gpt-model-{i}")).collect(),
            },
            StartableAgent {
                cli: "opencode".into(),
                billing: Billing::PerToken,
                room: None,
                room_levels: Vec::new(),
                models: (0..137)
                    .map(|i| format!("vendor/model-{i} $0.1/$0.5"))
                    .collect(),
            },
            StartableAgent {
                cli: "antigravity".into(),
                billing: Billing::Plan,
                room: Some(
                    "5h 100%, 7d 100%, Claude and GPT · 5h 100%, Claude and GPT · 7d 100%".into(),
                ),
                room_levels: vec![vec![Some(100), Some(100), Some(100), Some(100)]],
                models: vec![
                    "gemini-4-neon".into(),
                    "gemini-4-carbon".into(),
                    "claude-opus-5-5".into(),
                ],
            },
        ];
        let text = agents_text(&agents).expect("can-start text");
        assert!(text.contains("- claude (plan; left: "));
        assert!(text.contains("haiku (Haiku 5.5)"));
        assert!(text.contains("- codex (plan; left: Free: 30d 100%): gpt-model-0, gpt-model-1, gpt-model-2, gpt-model-3, +25 more"));
        assert!(text.contains("- opencode (paid per token): "));
        assert!(text.contains("+133 more"));
        assert!(text.contains("- antigravity (plan; left: 5h 100%"));
        assert!(text.contains("gemini-4-neon, gemini-4-carbon, claude-opus-5-5"));
        assert!(!text.contains("more agents"));
        assert!(text.contains("agent_list with models:"));
    }

    #[test]
    fn format_reading_orders_windows_and_marks_low_resets_and_stale() {
        let now = 1_800_000_000_000_u64;
        let reading = Reading {
            read_at: now - READING_STALE_AFTER_MS - 1,
            usage: Usage {
                provider: "antigravity".into(),
                plan: None,
                tier: None,
                windows: vec![
                    UsageWindow {
                        label: "7d".into(),
                        scope: None,
                        used_percent: 95.0,
                        resets_at: Some(UsageResetAt::Unix((now / 1000) + 3 * 3600 + 60)),
                        window_minutes: Some(10_080),
                    },
                    UsageWindow {
                        label: "5h".into(),
                        scope: None,
                        used_percent: 0.0,
                        resets_at: None,
                        window_minutes: Some(300),
                    },
                    UsageWindow {
                        label: "Claude and GPT · 7d".into(),
                        scope: Some("Claude and GPT".into()),
                        used_percent: 0.0,
                        resets_at: None,
                        window_minutes: Some(10_080),
                    },
                    UsageWindow {
                        label: "Claude and GPT · 5h".into(),
                        scope: Some("Claude and GPT".into()),
                        used_percent: 10.0,
                        resets_at: None,
                        window_minutes: Some(300),
                    },
                ],
                unavailable_reason: None,
                resets: None,
                light: false,
            },
        };
        assert_eq!(
            format_reading(Some(&reading), now),
            "5h 100%, 7d 5% (resets 3h), Claude and GPT · 5h 90%, Claude and GPT · 7d 100% (stale)"
        );
    }

    #[test]
    fn static_catalog_omits_volatile_room_and_quota_ticks_ignore_small_shifts() {
        let mut a = StartableAgent {
            cli: "claude".into(),
            billing: Billing::Plan,
            room: Some("Ultra: 7d 80%".into()),
            room_levels: vec![vec![Some(80)]],
            models: vec!["default (Opus 5.5)".into()],
        };
        let text_a = agents_text_with_room(&[a.clone()], false).unwrap();
        a.room = Some("Ultra: 7d 79%".into());
        let text_b = agents_text_with_room(&[a], false).unwrap();
        assert_eq!(text_a, text_b);
        assert!(!text_a.contains("left:"));

        let prev = vec![vec![Some(80)]];
        assert!(!quota_changed(Some(&prev), &[vec![Some(79)]]));
        assert!(!quota_changed(Some(&prev), &[vec![Some(66)]]));
        assert!(quota_changed(Some(&prev), &[vec![Some(65)]]));

        let near_25 = vec![vec![Some(26)]];
        assert!(quota_changed(Some(&near_25), &[vec![Some(24)]]));

        let near_10 = vec![vec![Some(11)]];
        assert!(quota_changed(Some(&near_10), &[vec![Some(9)]]));

        let near_zero = vec![vec![Some(2)]];
        assert!(quota_changed(Some(&near_zero), &[vec![Some(0)]]));
    }
}
