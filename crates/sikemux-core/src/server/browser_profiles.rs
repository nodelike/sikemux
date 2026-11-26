//! Account-level browser profiles: CRUD, Space/Playbook binding, and deterministic
//! Tab/Agent -> Playbook -> Space -> Global resolution.

use std::collections::BTreeMap;

use sikemux_calls::browser_profiles::{
    BrowserProfileDeleteAnswer, BrowserProfileDeleteParams, BrowserProfileResolveAnswer,
    BrowserProfileResolveParams, BrowserProfileSetGlobalParams, BrowserProfileSetProjectParams,
    BrowserProfileSetSpaceAnswer, BrowserProfileSetSpaceParams, BrowserProfileUpsertAnswer,
    BrowserProfileUpsertParams, BrowserProfilesView,
};
use sikemux_process::clock::epoch_ms;
use sikemux_view::browser_profiles::{
    default_profile, ensure_profiles, global_default, unique_slug, BrowserProfile, ProfileAccess,
    ProfileSource, ResolvedBrowserProfile, MAX_BROWSER_PROFILES, MAX_PROFILE_DESCRIPTION_CHARS,
    MAX_PROFILE_NAME_CHARS,
};
use sikemux_view::prefs::PreferenceChange;
use sikemux_view::workspace::SpaceInfo;

use crate::server::changed::{ChangedCore, What};
use crate::server::workspace_state::WorkspaceParts;
use crate::server::{preferences, workspace, Core, CoreError, CoreResult};

pub fn list(core: &Core) -> BrowserProfilesView {
    let prefs = core.preferences().get();
    let profiles = ensure_profiles(&prefs.browser_profiles);
    let global_default_id = global_default(&profiles, prefs.default_browser_profile.as_deref())
        .map(|profile| profile.id.clone())
        .unwrap_or_else(|| default_profile().id);
    BrowserProfilesView {
        profiles,
        global_default_id,
    }
}

pub fn resolve_for_project(
    profiles: &[BrowserProfile],
    default_id: Option<&str>,
    project_pins: &BTreeMap<String, String>,
    spaces: &[SpaceInfo],
    project_path: Option<&str>,
    project_name: Option<&str>,
    project_space_id: Option<&str>,
) -> ResolvedBrowserProfile {
    let profiles = ensure_profiles(profiles);
    let find = |id: &str| profiles.iter().find(|profile| profile.id == id).cloned();

    if let Some(folder) = project_path {
        if let Some(pinned_id) = project_pins.get(folder) {
            if let Some(profile) = find(pinned_id) {
                return ResolvedBrowserProfile {
                    profile,
                    source: ProfileSource::Playbook,
                    source_label: project_name.map(str::to_owned),
                };
            }
        }
    }

    if let Some(space_id) = project_space_id {
        if let Some(space) = spaces.iter().find(|space| space.id == space_id) {
            if let Some(profile_id) = space.browser_profile.as_deref() {
                if let Some(profile) = find(profile_id) {
                    return ResolvedBrowserProfile {
                        profile,
                        source: ProfileSource::Space,
                        source_label: Some(space.name.clone()),
                    };
                }
            }
        }
    }

    let profile = global_default(&profiles, default_id)
        .cloned()
        .unwrap_or_else(default_profile);
    ResolvedBrowserProfile {
        profile,
        source: ProfileSource::Global,
        source_label: None,
    }
}

pub fn upsert(
    core: &Core,
    params: BrowserProfileUpsertParams,
) -> CoreResult<BrowserProfileUpsertAnswer> {
    let name: String = params
        .name
        .trim()
        .chars()
        .take(MAX_PROFILE_NAME_CHARS)
        .collect();
    if name.is_empty() {
        return Err("profile name cannot be empty".into());
    }
    let prefs = core.preferences().get();
    let mut profiles = ensure_profiles(&prefs.browser_profiles);
    let saved = if let Some(id) = params.id.as_deref().filter(|id| !id.trim().is_empty()) {
        let Some(index) = profiles.iter().position(|profile| profile.id == id) else {
            return Err("that browser profile does not exist".into());
        };
        let existing_slug = profiles
            .get(index)
            .map(|profile| profile.slug.as_str())
            .unwrap_or_default();
        let desired_slug = params
            .slug
            .as_deref()
            .filter(|slug| !slug.trim().is_empty())
            .unwrap_or(existing_slug);
        let slug = unique_slug(desired_slug, &profiles, Some(id));
        let Some(entry) = profiles.get_mut(index) else {
            return Err("that browser profile does not exist".into());
        };
        entry.name = name;
        entry.slug = slug;
        if let Some(description) = params.description {
            entry.description = description
                .trim()
                .chars()
                .take(MAX_PROFILE_DESCRIPTION_CHARS)
                .collect();
        }
        if let Some(access) = params.access {
            entry.access = access;
        }
        entry.clone()
    } else {
        if profiles.len() >= MAX_BROWSER_PROFILES {
            return Err("this host already keeps as many browser profiles as it can".into());
        }
        let desired_slug = params
            .slug
            .as_deref()
            .filter(|slug| !slug.trim().is_empty())
            .unwrap_or(name.as_str());
        let slug = unique_slug(desired_slug, &profiles, None);
        let profile = BrowserProfile {
            id: uuid::Uuid::new_v4().to_string(),
            slug,
            name,
            description: params
                .description
                .unwrap_or_default()
                .trim()
                .chars()
                .take(MAX_PROFILE_DESCRIPTION_CHARS)
                .collect(),
            access: params.access.unwrap_or_default(),
            is_default_store: false,
            created_at: epoch_ms(),
        };
        profiles.push(profile.clone());
        profile
    };

    preferences::set(core, PreferenceChange::BrowserProfiles(profiles.clone()))?;

    if let Some(space_id) = params.link_space_id.as_deref().filter(|id| !id.is_empty()) {
        link_space(core, space_id, Some(saved.id.clone()))?;
    }

    let view = list(core);
    Ok(BrowserProfileUpsertAnswer {
        profile: saved,
        profiles: view.profiles,
        global_default_id: view.global_default_id,
    })
}

pub fn delete(
    core: &Core,
    params: BrowserProfileDeleteParams,
) -> CoreResult<BrowserProfileDeleteAnswer> {
    let prefs = core.preferences().get();
    let mut profiles = ensure_profiles(&prefs.browser_profiles);
    let Some(index) = profiles.iter().position(|profile| profile.id == params.id) else {
        return Err("that browser profile does not exist".into());
    };
    if profiles
        .get(index)
        .is_some_and(|profile| profile.is_default_store)
    {
        return Err(
            "the default browser profile cannot be deleted; clear its site data instead".into(),
        );
    }
    if profiles.len() <= 1 {
        return Err("at least one browser profile must remain".into());
    }
    profiles.remove(index);

    let next_default = if prefs.default_browser_profile.as_deref() == Some(params.id.as_str()) {
        global_default(&profiles, None).map(|profile| profile.id.clone())
    } else {
        prefs.default_browser_profile.clone()
    };

    let mut project_pins = prefs.project_browser_profiles.clone();
    project_pins.retain(|_, id| id != &params.id);

    preferences::set(core, PreferenceChange::BrowserProfiles(profiles.clone()))?;
    if next_default != prefs.default_browser_profile {
        preferences::set(core, PreferenceChange::DefaultBrowserProfile(next_default))?;
    }
    if project_pins != prefs.project_browser_profiles {
        preferences::set(core, PreferenceChange::ProjectBrowserProfiles(project_pins))?;
    }

    let mut spaces = core.workspaces().view().spaces;
    let mut spaces_changed = false;
    for space in &mut spaces {
        if space.browser_profile.as_deref() == Some(params.id.as_str()) {
            space.browser_profile = None;
            spaces_changed = true;
        }
    }
    if spaces_changed {
        workspace::set_spaces(core, sikemux_calls::workspace::SpacesParams { spaces })?;
        core.changed(What::ViewNow);
    }

    let view = list(core);
    Ok(BrowserProfileDeleteAnswer {
        deleted_id: params.id,
        profiles: view.profiles,
        global_default_id: view.global_default_id,
    })
}

pub fn set_global(
    core: &Core,
    params: BrowserProfileSetGlobalParams,
) -> CoreResult<BrowserProfilesView> {
    let prefs = core.preferences().get();
    let profiles = ensure_profiles(&prefs.browser_profiles);
    if !profiles.iter().any(|profile| profile.id == params.id) {
        return Err("that browser profile does not exist".into());
    }
    preferences::set(
        core,
        PreferenceChange::DefaultBrowserProfile(Some(params.id)),
    )?;
    Ok(list(core))
}

pub fn set_space(
    core: &Core,
    params: BrowserProfileSetSpaceParams,
) -> CoreResult<BrowserProfileSetSpaceAnswer> {
    let spaces = core.workspaces().view().spaces;
    let Some(space) = spaces.iter().find(|space| space.id == params.space_id) else {
        return Err("that space does not exist".into());
    };
    let space_name = space.name.clone();
    let prefs = core.preferences().get();
    let profiles = ensure_profiles(&prefs.browser_profiles);

    if params.create_matching {
        if let Some(existing) = profiles
            .iter()
            .find(|profile| profile.name.eq_ignore_ascii_case(&space_name))
            .cloned()
        {
            link_space(core, &params.space_id, Some(existing.id.clone()))?;
            return Ok(BrowserProfileSetSpaceAnswer {
                space_id: params.space_id,
                profile: Some(existing),
                created: false,
            });
        }
        let created = upsert(
            core,
            BrowserProfileUpsertParams {
                id: None,
                name: space_name,
                slug: None,
                description: None,
                access: None,
                link_space_id: Some(params.space_id.clone()),
            },
        )?;
        return Ok(BrowserProfileSetSpaceAnswer {
            space_id: params.space_id,
            profile: Some(created.profile),
            created: true,
        });
    }

    let profile = match params.profile_id.as_deref().filter(|id| !id.is_empty()) {
        Some(id) => {
            let found = profiles
                .iter()
                .find(|profile| profile.id == id)
                .cloned()
                .ok_or_else(|| CoreError::from("that browser profile does not exist"))?;
            link_space(core, &params.space_id, Some(found.id.clone()))?;
            Some(found)
        }
        None => {
            link_space(core, &params.space_id, None)?;
            None
        }
    };

    Ok(BrowserProfileSetSpaceAnswer {
        space_id: params.space_id,
        profile,
        created: false,
    })
}

pub fn set_project(core: &Core, params: BrowserProfileSetProjectParams) -> CoreResult<()> {
    let prefs = core.preferences().get();
    let profiles = ensure_profiles(&prefs.browser_profiles);
    let mut pins = prefs.project_browser_profiles.clone();
    match params.profile_id.as_deref().filter(|id| !id.is_empty()) {
        Some(id) => {
            if !profiles.iter().any(|profile| profile.id == id) {
                return Err("that browser profile does not exist".into());
            }
            pins.insert(params.project, id.to_owned());
        }
        None => {
            pins.remove(&params.project);
        }
    }
    preferences::set(core, PreferenceChange::ProjectBrowserProfiles(pins))?;
    Ok(())
}

pub fn resolve(
    core: &Core,
    params: BrowserProfileResolveParams,
) -> CoreResult<BrowserProfileResolveAnswer> {
    let prefs = core.preferences().get();
    let profiles = ensure_profiles(&prefs.browser_profiles);
    let global_default_id = global_default(&profiles, prefs.default_browser_profile.as_deref())
        .map(|profile| profile.id.clone())
        .unwrap_or_else(|| default_profile().id);

    let mut workspace = core.workspaces().view();
    (core.workspaces().reaches).follow(&mut workspace.projects, &prefs.project_roots);

    let project_info = params
        .project
        .as_deref()
        .filter(|path| !path.is_empty())
        .and_then(|folder| {
            let target = std::path::Path::new(folder);
            workspace
                .projects
                .iter()
                .find(|project| project.path == target || project.path.to_string_lossy() == folder)
        });

    let inherited = resolve_for_project(
        &profiles,
        prefs.default_browser_profile.as_deref(),
        &prefs.project_browser_profiles,
        &workspace.spaces,
        params
            .project
            .as_deref()
            .filter(|path| !path.is_empty())
            .or_else(|| project_info.map(|project| project.path.to_str().unwrap_or_default())),
        project_info.map(|project| project.name.as_str()),
        project_info.and_then(|project| project.space.as_deref()),
    );

    let find_by_handle = |handle: &str| {
        profiles
            .iter()
            .find(|profile| {
                profile.id == handle
                    || profile.slug.eq_ignore_ascii_case(handle)
                    || profile.name.eq_ignore_ascii_case(handle)
            })
            .cloned()
    };

    let agent_name = params.agent_id.as_deref().and_then(|agent_id| {
        core.names()
            .get(agent_id)
            .map(|entry| format!("@{}", entry.name))
    });

    let resolved = if let Some(explicit) = params
        .explicit_profile
        .as_deref()
        .map(str::trim)
        .filter(|handle| !handle.is_empty())
    {
        let Some(profile) = find_by_handle(explicit) else {
            let valid: Vec<&str> = profiles
                .iter()
                .map(|profile| profile.slug.as_str())
                .collect();
            return Err(format!(
                "unknown browser profile \"{explicit}\"; available profiles: {}",
                valid.join(", ")
            )
            .into());
        };
        if params.agent_id.is_some() {
            ResolvedBrowserProfile {
                profile,
                source: ProfileSource::AgentOverride,
                source_label: agent_name.clone(),
            }
        } else {
            ResolvedBrowserProfile {
                profile,
                source: ProfileSource::TabOverride,
                source_label: None,
            }
        }
    } else if let Some(override_id) = params.tab_override.as_deref().filter(|id| !id.is_empty()) {
        if let Some(profile) = find_by_handle(override_id) {
            if let Some(by) = params.tab_override_by.filter(|by| !by.is_empty()) {
                ResolvedBrowserProfile {
                    profile,
                    source: ProfileSource::AgentOverride,
                    source_label: Some(by),
                }
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

    if params.agent_id.is_some() {
        match resolved.profile.access {
            ProfileAccess::Standard => {}
            ProfileAccess::ManualOnly => {
                return Err(format!(
                    "browser profile \"{}\" ({}) is set to manual only; ask the person to open or take over this page directly",
                    resolved.profile.name, resolved.profile.slug
                )
                .into());
            }
            ProfileAccess::AskOutsideProject => {
                if resolved.profile.id != inherited.profile.id {
                    let target = params
                        .url
                        .as_deref()
                        .filter(|url| !url.is_empty())
                        .unwrap_or("this page");
                    return Err(format!(
                        "browser profile \"{}\" ({}) is protected outside its assigned Space/Playbook (this project defaults to \"{}\"). Ask the person before opening {} in it.",
                        resolved.profile.name, resolved.profile.slug, inherited.profile.name, target
                    )
                    .into());
                }
            }
        }
    }

    Ok(BrowserProfileResolveAnswer {
        resolved,
        inherited,
        profiles,
        global_default_id,
    })
}

pub fn brief_line(core: &Core, project: &str) -> Option<String> {
    let prefs = core.preferences().get();
    let profiles = ensure_profiles(&prefs.browser_profiles);
    if profiles.len() <= 1 {
        return None;
    }
    let mut workspace = core.workspaces().view();
    (core.workspaces().reaches).follow(&mut workspace.projects, &prefs.project_roots);
    let project_info = workspace
        .projects
        .iter()
        .find(|entry| entry.path.to_string_lossy() == project);
    let default = resolve_for_project(
        &profiles,
        prefs.default_browser_profile.as_deref(),
        &prefs.project_browser_profiles,
        &workspace.spaces,
        Some(project),
        project_info.map(|entry| entry.name.as_str()),
        project_info.and_then(|entry| entry.space.as_deref()),
    );
    let entries: Vec<String> = profiles
        .iter()
        .map(|profile| {
            let mut tags = Vec::new();
            if profile.id == default.profile.id {
                tags.push("default");
            }
            match profile.access {
                ProfileAccess::Standard => {}
                ProfileAccess::AskOutsideProject => tags.push("ask outside project"),
                ProfileAccess::ManualOnly => tags.push("manual only"),
            }
            let detail = match (tags.is_empty(), profile.description.trim().is_empty()) {
                (true, true) => String::new(),
                (false, true) => format!(" ({})", tags.join(", ")),
                (true, false) => format!(" ({})", crate::text::one_line(&profile.description)),
                (false, false) => format!(
                    " ({} — {})",
                    tags.join(", "),
                    crate::text::one_line(&profile.description)
                ),
            };
            format!("{}{detail}", profile.slug)
        })
        .collect();
    Some(format!(
        "Browser profiles: {}. Pass profile to browser_navigate when another account is needed.",
        entries.join("; ")
    ))
}

fn link_space(core: &Core, space_id: &str, profile_id: Option<String>) -> CoreResult<()> {
    let mut spaces = core.workspaces().view().spaces;
    let Some(space) = spaces.iter_mut().find(|space| space.id == space_id) else {
        return Err("that space does not exist".into());
    };
    space.browser_profile = profile_id;
    workspace::set_spaces(core, sikemux_calls::workspace::SpacesParams { spaces })?;
    core.changed(What::ViewNow);
    Ok(())
}
