//! The production places a project's own files name: its deploy config, its workflows and the
//! commands its sikemux.json lists as shipping. Agents can write these files, so what is found
//! here only ever adds a production place; it never frees anything.

use std::path::Path;

use sikemux_view::places::Recognise;

use super::judge::stage_place;
use super::place::{new_place, says_production, Place, GITHUB_RELEASES};

/// Every production place the files of the project at `root` name.
pub(crate) fn in_files(root: &Path) -> Vec<Place> {
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
            let mut place = stage_place(tool, &name, Some(file.to_owned()));
            place.shown.production = true;
            found.push(place);
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
    let mut seen = std::collections::HashSet::new();
    found.retain(|place| seen.insert(place.shown.id.clone()));
    found
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

/// GitHub's releases, when a workflow publishes one, and each deployment environment a
/// workflow names that reads as production. Where a workflow tells its tags apart, its stable
/// releases are where any other release lands too.
fn workflows(root: &Path, found: &mut Vec<Place>) {
    let Ok(entries) = std::fs::read_dir(root.join(".github/workflows")) else {
        return;
    };
    let mut files: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
    files.sort();
    let mut releases: Option<(String, Vec<Recognise>)> = None;
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
            let (_, facts) = releases.get_or_insert_with(|| (shown.clone(), Vec::new()));
            facts.push(words.clone());
        }
        let production: Vec<&String> = (read.environments.iter())
            .filter(|name| says_production(name))
            .collect();
        if read.deploys && production.is_empty() && !read.publishes {
            found.push(new_place(
                &format!("The {file} workflow on GitHub"),
                true,
                vec![words.clone()],
                Some(shown.clone()),
            ));
        }
        for environment in production {
            let name = format!("{environment} on GitHub");
            found.push(new_place(
                &name,
                true,
                vec![words.clone()],
                Some(shown.clone()),
            ));
        }
    }
    if let Some((file, mut facts)) = releases {
        facts.insert(
            0,
            Recognise::Package {
                registry: GITHUB_RELEASES.to_owned(),
            },
        );
        match stable.and_then(|at| found.get_mut(at)) {
            Some(stable) => stable.recognise.extend(facts),
            None => found.push(new_place("Releases on GitHub", true, facts, Some(file))),
        }
    }
}

/// The tag patterns a workflow runs on, from its `on: push: tags:` filter.
fn tag_patterns(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let Some(at) = lines
        .iter()
        .position(|line| line.trim_start().starts_with("tags:"))
    else {
        return Vec::new();
    };
    let clean = |item: &str| item.trim().trim_matches(['"', '\'']).to_owned();
    let inline = lines[at].trim_start().trim_start_matches("tags:").trim();
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
    (lines[at + 1..].iter())
        .map(|line| line.trim())
        .take_while(|line| line.starts_with("- "))
        .map(|line| clean(&line[2..]))
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
