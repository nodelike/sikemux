//! The production places a project's own files name: its deploy config, its workflows and the
//! commands its sikemux.json lists as shipping. Agents can write these files, so what is found
//! here only ever adds a production place; it never frees anything.

use std::path::Path;

use sikemux_view::places::Recognise;

use super::place::{new_place, says_not_production, says_production, Place, GITHUB_RELEASES};

/// Every production place the files of the project at `root` name.
pub fn in_files(root: &Path) -> Vec<Place> {
    let mut found = Vec::new();
    vercel(root, &mut found);
    for (file, tool) in [
        ("fly.toml", "Fly"),
        ("wrangler.toml", "Cloudflare"),
        ("wrangler.json", "Cloudflare"),
        ("wrangler.jsonc", "Cloudflare"),
        (".firebaserc", "Firebase"),
    ] {
        let Some(text) = read(root, file) else {
            continue;
        };
        if let Some(name) = crate::ship::app_named(tool, &text) {
            if !says_not_production(&name) {
                found.push(production_stage(tool, &name, file));
            }
        }
        if tool == "Cloudflare" {
            for env in wrangler_envs(&text) {
                if !says_not_production(&env) {
                    found.push(production_stage("Cloudflare", &env, file));
                }
            }
        }
    }
    for file in ["railway.json", "railway.toml"] {
        if read(root, file).is_some() {
            found.push(new_place(
                &format!("{} on Railway", folder(root)),
                true,
                vec![Recognise::Stage {
                    tool: "Railway".to_owned(),
                    name: "default".to_owned(),
                }],
                Some(file.to_owned()),
            ));
            break;
        }
    }
    workflows(root, &mut found);
    for command in crate::git_guard::shim::project_ships(root) {
        let words = Recognise::Command {
            words: command.clone(),
        };
        found.push(new_place(
            &command,
            true,
            vec![words],
            Some(crate::tasks::config::CONFIG_FILE.to_owned()),
        ));
    }
    let mut places = Vec::new();
    for place in found {
        super::keep(&mut places, place);
    }
    places
}

fn production_stage(tool: &str, name: &str, file: &str) -> Place {
    let fact = Recognise::Stage {
        tool: tool.to_owned(),
        name: name.to_owned(),
    };
    new_place(
        &format!("{name} on {tool}"),
        true,
        vec![fact],
        Some(file.to_owned()),
    )
}

/// Environment names declared under `[env.<name>]` in `wrangler.toml` or `"env": { "<name>": ... }`
/// in `wrangler.json`/`wrangler.jsonc`.
fn wrangler_envs(text: &str) -> Vec<String> {
    let mut envs = Vec::new();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(map) = value.get("env").and_then(serde_json::Value::as_object) {
            envs.extend(map.keys().cloned());
        }
        return envs;
    }
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed
            .strip_prefix("[env.")
            .and_then(|rest| rest.strip_suffix(']'))
        {
            let name = rest.split('.').next().unwrap_or_default().trim();
            let name = name.trim_matches(['"', '\'']);
            if !name.is_empty() && !envs.iter().any(|seen| seen == name) {
                envs.push(name.to_owned());
            }
        }
    }
    envs
}

fn read(root: &Path, file: &str) -> Option<String> {
    std::fs::read_to_string(root.join(file)).ok()
}

fn folder(root: &Path) -> String {
    root.file_name().map_or_else(
        || "The app".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Vercel's production, named by the first domain its config aliases or the project it is
/// linked to.
fn vercel(root: &Path, found: &mut Vec<Place>) {
    let config = read(root, "vercel.json");
    let linked = read(root, ".vercel/project.json");
    let Some(file) = (config.as_ref().map(|_| "vercel.json"))
        .or_else(|| linked.as_ref().map(|_| ".vercel/project.json"))
    else {
        return;
    };
    let json = |text: &Option<String>| {
        text.as_deref()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
    };
    let (config, linked) = (json(&config), json(&linked));
    let alias = config.as_ref().and_then(|value| {
        (value["alias"].as_array())
            .and_then(|aliases| aliases.first()?.as_str().map(str::to_owned))
            .or_else(|| value["alias"].as_str().map(str::to_owned))
    });
    let project = (linked.as_ref())
        .and_then(|value| value["projectName"].as_str().map(str::to_owned))
        .or_else(|| config.as_ref()?["name"].as_str().map(str::to_owned));
    let name = alias.or(project).unwrap_or_else(|| folder(root));
    found.push(new_place(
        &format!("{name} on Vercel"),
        true,
        vec![Recognise::VercelTarget { production: true }],
        Some(file.to_owned()),
    ));
}

/// The workflow of the project at `root` that starts the workflow `file` by itself, on a
/// schedule or when another run ends.
pub fn starter_of(root: &Path, file: &str) -> Option<String> {
    let starts = format!("gh workflow run {file}");
    let entries = std::fs::read_dir(root.join(".github/workflows")).ok()?;
    entries.flatten().find_map(|entry| {
        let text = std::fs::read_to_string(entry.path()).ok()?;
        let lines = || text.lines().map(str::trim);
        let unattended = lines().any(|line| matches!(line, "schedule:" | "workflow_run:"));
        let starts_it = lines().any(|line| {
            (line.split(starts.as_str()).skip(1))
                .any(|after| after.is_empty() || after.starts_with(char::is_whitespace))
        });
        (unattended && starts_it).then(|| entry.file_name().to_string_lossy().into_owned())
    })
}

/// Workflows in `root` (in the checkout or committed at `HEAD` / `origin/*`) that deploy to
/// production when a branch is pushed.
pub fn workflow_push_deploys(root: &Path) -> Vec<super::store::PushDeploy> {
    let mut found = Vec::new();
    let mut collect =
        |file: &str, mut text: String, load_called: &dyn Fn(&str) -> Option<String>| {
            let Some(trigger) = crate::ship::push_trigger(&text) else {
                return;
            };
            for called in crate::ship::called_workflows(&text.clone()) {
                if let Some(more) = load_called(&called) {
                    text.push('\n');
                    text.push_str(&more);
                }
            }
            let read = crate::ship::read_workflow(&text);
            let prod_env = read.environments.iter().any(|name| says_production(name));
            let non_dev_env = read.environments.is_empty()
                || read
                    .environments
                    .iter()
                    .any(|name| !says_not_production(name));
            let deploys_production = prod_env
                || ((read.deploys || read.publishes) && non_dev_env && !says_not_production(file));
            if !deploys_production {
                return;
            }
            let deploy = super::store::PushDeploy {
                file: file.to_owned(),
                environments: read.environments,
                branches: trigger.branches,
                branches_ignore: trigger.branches_ignore,
                paths: trigger.paths,
                paths_ignore: trigger.paths_ignore,
            };
            merge_push_deploy(&mut found, deploy);
        };
    if let Ok(entries) = std::fs::read_dir(root.join(".github/workflows")) {
        let mut files: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
        files.sort();
        for path in files {
            let Some(file) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !file.ends_with(".yml") && !file.ends_with(".yaml") {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                collect(file, text, &|called| {
                    std::fs::read_to_string(root.join(called)).ok()
                });
            }
        }
    }
    let git = || crate::git::Git::quick(root);
    for reference in ["HEAD", "origin/HEAD", "origin/main", "origin/master"] {
        let Ok(listed) =
            git().trimmed(&["ls-tree", "--name-only", reference, ".github/workflows/"])
        else {
            continue;
        };
        for path in listed.lines() {
            let Some(file) = path.rsplit('/').next() else {
                continue;
            };
            if !file.ends_with(".yml") && !file.ends_with(".yaml") {
                continue;
            }
            let Ok(text) = git().trimmed(&["show", &format!("{reference}:{path}")]) else {
                continue;
            };
            collect(file, text, &|called| {
                git()
                    .trimmed(&["show", &format!("{reference}:{called}")])
                    .ok()
            });
        }
    }
    found
}

/// Keeps `deploy` in `list`, never narrowing a workflow already seen without path filters.
pub fn merge_push_deploy(
    list: &mut Vec<super::store::PushDeploy>,
    deploy: super::store::PushDeploy,
) {
    if let Some(kept) = list.iter_mut().find(|kept| kept.file == deploy.file) {
        if deploy.paths.is_empty() {
            kept.paths.clear();
            kept.paths_ignore.clear();
        }
        if deploy.branches.is_empty() {
            kept.branches.clear();
        }
        for env in deploy.environments {
            if !kept.environments.contains(&env) {
                kept.environments.push(env);
            }
        }
        return;
    }
    list.push(deploy);
}

/// Whether a push to the branch of the project at `root` deploys it to production on Vercel:
/// the project is a Vercel one and its config doesn't turn off deploys from Git, in the checkout
/// or as committed or last pushed there.
pub fn vercel_deploys_pushes(root: &Path) -> bool {
    let committed = |reference: &str| {
        let file = format!("{reference}:vercel.json");
        crate::git::Git::quick(root).trimmed(&["show", &file]).ok()
    };
    let deploys = |config: Option<String>| {
        config.is_some_and(|text| {
            !turns_off_deploys(&serde_json::from_str(&text).unwrap_or_default())
        })
    };
    let here = read(root, "vercel.json");
    if here.is_none() && read(root, ".vercel/project.json").is_some() {
        return true;
    }
    deploys(here)
        || ["HEAD", "origin/HEAD", "origin/main", "origin/master"]
            .iter()
            .any(|reference| deploys(committed(reference)))
}

fn turns_off_deploys(config: &serde_json::Value) -> bool {
    let turned_off = match config.pointer("/git/deploymentEnabled") {
        Some(serde_json::Value::Bool(enabled)) => !enabled,
        Some(serde_json::Value::Object(branches)) => {
            let mut own = ["main", "master", "*", "**"]
                .into_iter()
                .filter_map(|name| branches.get(name))
                .peekable();
            own.peek().is_some() && own.all(|on| on == false)
        }
        _ => false,
    };
    turned_off || config.pointer("/github/enabled") == Some(&serde_json::Value::Bool(false))
}

/// Each deployment environment a workflow names that reads as production, and the stable
/// releases where a workflow tells its tags apart, which any other release a workflow
/// publishes lands with.
fn workflows(root: &Path, found: &mut Vec<Place>) {
    let Ok(entries) = std::fs::read_dir(root.join(".github/workflows")) else {
        return;
    };
    let mut files: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
    files.sort();
    let mut releases: Option<Vec<Recognise>> = None;
    let mut stable: Option<usize> = None;
    for path in files {
        let Ok(mut text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for called in crate::ship::called_workflows(&text.clone()) {
            if let Ok(more) = std::fs::read_to_string(root.join(called)) {
                text.push('\n');
                text.push_str(&more);
            }
        }
        let file = path.file_name().unwrap_or_default().to_string_lossy();
        let shown = format!(".github/workflows/{file}");
        let words = Recognise::Command {
            words: format!("gh workflow run {file}"),
        };
        let read = crate::ship::read_workflow(&text);
        stable = tag_places(&text, &shown, found).or(stable);
        if read.publishes {
            releases.get_or_insert_with(Vec::new).push(words.clone());
        }
        let production: Vec<&String> = (read.environments.iter())
            .filter(|name| says_production(name) || (read.deploys && !says_not_production(name)))
            .collect();
        for environment in &production {
            let name = format!("{environment} on GitHub");
            found.push(new_place(
                &name,
                true,
                vec![words.clone()],
                Some(shown.clone()),
            ));
        }
        if production.is_empty()
            && read.environments.is_empty()
            && read.deploys
            && !read.publishes
            && !says_not_production(&file)
        {
            found.push(new_place(
                &format!("The {file} workflow on GitHub"),
                true,
                vec![words.clone()],
                Some(shown.clone()),
            ));
        }
    }
    if let Some(mut facts) = releases {
        facts.insert(
            0,
            Recognise::Package {
                registry: GITHUB_RELEASES.to_owned(),
            },
        );
        if let Some(stable) = stable.and_then(|at| found.get_mut(at)) {
            stable.recognise.extend(facts);
        } else {
            found.push(new_place("Releases on GitHub", true, facts, None));
        }
    }
}

/// The tag patterns a workflow runs on, from its `on: push: tags:` filter.
fn tag_patterns(text: &str) -> Vec<String> {
    let mut lines = text
        .lines()
        .skip_while(|line| !line.trim_start().starts_with("tags:"));
    let Some(tagged) = lines.next() else {
        return Vec::new();
    };
    let clean = |item: &str| item.trim().trim_matches(['"', '\'']).to_owned();
    let inline = tagged.trim_start().trim_start_matches("tags:").trim();
    if let Some(list) = inline
        .strip_prefix('[')
        .and_then(|list| list.strip_suffix(']'))
    {
        return list
            .split(',')
            .map(clean)
            .filter(|item| !item.is_empty())
            .collect();
    }
    lines
        .map(str::trim)
        .map_while(|line| line.strip_prefix("- "))
        .map(clean)
        .collect()
}

/// The places a workflow that runs on pushed tags makes of them. Where its jobs tell some
/// tags apart by a word in their name, as `contains(github.ref_name, '-nightly.')` does, those
/// tags are a place of their own, done again and again, and the rest another. It answers where
/// in `found` the place of the tags not told apart is.
fn tag_places(text: &str, file: &str, found: &mut Vec<Place>) -> Option<usize> {
    let patterns = tag_patterns(text);
    if patterns.is_empty() {
        return None;
    }
    let read = crate::ship::read_workflow(text);
    if !read.publishes && !read.deploys {
        return None;
    }
    let apart = text
        .split("contains(github.ref_name,")
        .nth(1)
        .and_then(|rest| {
            let quoted = rest.trim_start().strip_prefix('\'')?;
            Some(quoted.split('\'').next()?.to_owned())
        });
    let tags = |pattern: &str, except: Option<String>| Recognise::Tag {
        pattern: pattern.to_owned(),
        except,
    };
    let Some(word) = apart.filter(|word| !word.is_empty()) else {
        let facts = patterns.iter().map(|pattern| tags(pattern, None)).collect();
        found.push(new_place(
            "Releases on GitHub",
            true,
            facts,
            Some(file.to_owned()),
        ));
        return Some(found.len() - 1);
    };
    let marked = format!("*{word}*");
    let title = word.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    let (marked_name, rest_name) = match title.eq_ignore_ascii_case("nightly") {
        true => (
            "Nightly releases on GitHub".to_owned(),
            "Stable releases on GitHub".to_owned(),
        ),
        false => (
            format!("{title} releases on GitHub"),
            "Other releases on GitHub".to_owned(),
        ),
    };
    let mut recurring = new_place(
        &marked_name,
        true,
        vec![tags(&marked, None)],
        Some(file.to_owned()),
    );
    recurring.recurs = true;
    found.push(recurring);
    let rest = patterns
        .iter()
        .map(|pattern| tags(pattern, Some(marked.clone())))
        .collect();
    found.push(new_place(&rest_name, true, rest, Some(file.to_owned())));
    Some(found.len() - 1)
}
