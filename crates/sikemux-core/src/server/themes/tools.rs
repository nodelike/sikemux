//! What an agent restyles Sikemux itself with: `theme_read` and `theme_set`.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Map, Value};
use sikemux_corekit::cli::manifest::GUIDE;
use sikemux_corekit::themes::{checked, custom_id, find, ink_reads, summaries, MOST_CUSTOM_THEMES};
use sikemux_view::prefs::{Contrast, Motion, PreferenceChange, Preferences, Scrollbars};
use sikemux_view::theme::{Theme, ThemeChanges, ThemeSource};

use crate::cli::protocol::HarnessRequest;
use crate::server::parts::Parts;
use crate::server::workspace_state::WorkspaceParts;
use crate::server::Core;

const TOPIC: &str = "## theme:";

pub(crate) fn read(core: &Core, request: &HarnessRequest) -> Result<Value, String> {
    let preferences = core.preferences().get();
    let omarchy = core.look().omarchy();
    let all = summaries(&preferences, omarchy.as_ref());
    let ghostty = all
        .iter()
        .filter(|theme| theme.source == ThemeSource::Ghostty)
        .count();
    let themes: Vec<Value> = (all.iter())
        .filter(|theme| theme.source != ThemeSource::Ghostty)
        .map(|theme| {
            json!({
                "id": theme.id,
                "name": theme.name,
                "dark": theme.dark,
                "yours": theme.source == ThemeSource::Custom,
            })
        })
        .collect();
    let theme = match request.params.get("theme").and_then(Value::as_str) {
        Some(id) => find(&preferences, omarchy.as_ref(), id)
            .ok_or_else(|| format!("there is no theme {id}; theme_read lists them"))?,
        None => core.look().palette().theme,
    };
    Ok(json!({
        "wearing": core.look().palette().theme.id,
        "adjust": {
            "scrollbars": preferences.scrollbars,
            "contrast": preferences.contrast,
            "motion": preferences.motion,
        },
        "themes": themes,
        "ghostty": format!(
            "{ghostty} more from Ghostty's catalogue, each with the id ghostty- and its name in kebab case"
        ),
        "theme": theme,
        "guide": topic(),
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetParams {
    theme: Option<String>,
    save: Option<ThemeChanges>,
    adjust: Option<Adjust>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Adjust {
    scrollbars: Option<Scrollbars>,
    contrast: Option<Contrast>,
    motion: Option<Motion>,
}

pub(crate) async fn set(core: &Arc<Core>, request: HarnessRequest) -> Result<Value, String> {
    let params: SetParams = serde_json::from_value(request.params.clone())
        .map_err(|error| format!("theme_set takes theme, save and adjust: {error}"))?;
    let before = core.preferences().get();
    let omarchy = core.look().omarchy();
    let mut changes = Vec::new();
    let mut clamped = Vec::new();
    let wear = match (params.theme, params.save) {
        (Some(_), Some(_)) => return Err("give theme or save, not both".into()),
        (Some(id), None) => {
            find(&before, omarchy.as_ref(), &id)
                .ok_or_else(|| format!("there is no theme {id}; theme_read lists them"))?;
            Some(id)
        }
        (None, Some(save)) => {
            let (theme, notes) = saved(&before, omarchy.as_ref(), save)?;
            clamped = notes;
            let mut themes = before.custom_themes.clone();
            themes.retain(|kept| kept.id != theme.id);
            if themes.len() >= MOST_CUSTOM_THEMES {
                return Err(format!(
                    "the person keeps {MOST_CUSTOM_THEMES} themes already; save over one of the agent-made ones by its name"
                ));
            }
            let id = theme.id.clone();
            themes.push(theme);
            changes.push(PreferenceChange::CustomThemes(themes));
            Some(id)
        }
        (None, None) => None,
    };
    changes.extend(wear.map(PreferenceChange::ThemeId));
    let adjust = params.adjust.unwrap_or_default();
    changes.extend(adjust.scrollbars.map(PreferenceChange::Scrollbars));
    changes.extend(adjust.contrast.map(PreferenceChange::Contrast));
    changes.extend(adjust.motion.map(PreferenceChange::Motion));
    if changes.is_empty() {
        return Err("give a theme to wear, one to save, or an adjustment".into());
    }
    for change in changes {
        crate::server::preferences::set(core, change).map_err(|error| error.to_string())?;
    }
    let worn = core.look().palette().theme;
    let undo = json!({
        "theme": before.theme_id,
        "adjust": {
            "scrollbars": before.scrollbars,
            "contrast": before.contrast,
            "motion": before.motion,
        },
    });
    let agent = (request.agent_id.as_deref()).map(|agent| core.names().name_or_id(agent));
    let mut notice = request;
    notice.params = json!({ "agent": agent, "theme": worn.name, "undo": undo });
    let mut answer = Map::new();
    answer.insert("wearing".into(), json!(worn.id));
    answer.insert("clamped".into(), json!(clamped));
    answer.insert("undo".into(), undo);
    match crate::server::harness::forward(core, notice).await {
        Ok(Value::Object(shown)) => answer.extend(shown),
        Ok(_) => {}
        Err(error) => {
            answer.insert("picture".into(), json!(format!("none: {error}")));
        }
    }
    Ok(Value::Object(answer))
}

/// A theme saved from `save`: a base theme with the changes laid over it, checked.
fn saved(
    preferences: &Preferences,
    omarchy: Option<&Theme>,
    mut save: ThemeChanges,
) -> Result<(Theme, Vec<String>), String> {
    let name = save
        .name
        .take()
        .ok_or("save needs a name for the new theme")?;
    let base_id = save
        .base
        .take()
        .unwrap_or_else(|| preferences.theme_id.clone());
    let mut theme = find(preferences, omarchy, &base_id)
        .ok_or_else(|| format!("there is no theme {base_id} to start from"))?;
    save.lay_over(&mut theme);
    theme.id = custom_id(&name);
    theme.name = name;
    let (theme, clamped) = checked(theme)?;
    ink_reads(&theme)?;
    Ok((theme, clamped))
}

/// The guide's theme topic.
fn topic() -> &'static str {
    let Some(start) = GUIDE.find(TOPIC) else {
        return "";
    };
    let rest = &GUIDE[start..];
    let end = rest[TOPIC.len()..]
        .find("\n## ")
        .map_or(rest.len(), |end| end + TOPIC.len() + 1);
    rest[..end].trim_end()
}
