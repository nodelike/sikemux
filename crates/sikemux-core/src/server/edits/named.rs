//! Files a commit's new lines name that the commit itself lacks: a Rust
//! `mod x;` whose file is not in it, or a relative import whose module is
//! not. Committing such lines before their file breaks every build of the
//! commit, which agents sharing a checkout did over and over.

use std::collections::HashSet;

/// For each line added to `path`, given its old and new text, the files
/// `tree` (every path in the commit) would need and lacks, as "what names
/// it: what is missing".
pub(super) fn missing(path: &str, old: &[u8], new: &[u8], tree: &HashSet<String>) -> Vec<String> {
    let (Ok(old), Ok(new)) = (std::str::from_utf8(old), std::str::from_utf8(new)) else {
        return Vec::new();
    };
    let before: HashSet<&str> = old.lines().map(str::trim).collect();
    let lines: Vec<&str> = new.lines().map(str::trim).collect();
    let added = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| !before.contains(*line));
    let mut lacking = Vec::new();
    for (at, line) in added {
        let placed = at
            .checked_sub(1)
            .is_some_and(|above| lines[above].starts_with("#[path"));
        let wants = if placed {
            None
        } else if path.ends_with(".rs") {
            rust_module(path, line, tree).map(|(name, files)| (format!("`mod {name};`"), files))
        } else if is_script(path) {
            relative_import(path, line).map(|(spec, files)| (format!("an import of {spec}"), files))
        } else {
            None
        };
        if let Some((said, files)) = wants {
            if !files.iter().any(|file| tree.contains(file)) {
                lacking.push(format!(
                    "{path} adds {said}, but {} is not in this commit",
                    files[0]
                ));
            }
        }
    }
    lacking
}

/// Whether [`missing`] looks at what lines added to `path` name.
pub(super) fn looks_at(path: &str) -> bool {
    path.ends_with(".rs") || is_script(path)
}

fn is_script(path: &str) -> bool {
    [".ts", ".tsx", ".js", ".jsx", ".mjs"]
        .iter()
        .any(|extension| path.ends_with(extension))
}

fn folder(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(folder, _)| folder)
}

fn joined(folder: &str, name: &str) -> String {
    match folder {
        "" => name.to_owned(),
        folder => format!("{folder}/{name}"),
    }
}

/// Whether `path` is a crate of its own, as a file straight in a package's
/// `tests`, `examples`, `benches` or `src/bin` is, whose modules sit beside it.
fn is_crate_root(path: &str, tree: &HashSet<String>) -> bool {
    let dir = folder(path);
    let package = match dir
        .rsplit_once('/')
        .map_or(("", dir), |(up, name)| (up, name))
    {
        (up, "tests" | "examples" | "benches") => up,
        (up, "bin") => match up
            .rsplit_once('/')
            .map_or(("", up), |(above, name)| (above, name))
        {
            (above, "src") => above,
            _ => return false,
        },
        _ => return false,
    };
    tree.contains(&joined(package, "Cargo.toml"))
}

/// The module a `mod name;` line declares, and the files that could hold it.
fn rust_module(path: &str, line: &str, tree: &HashSet<String>) -> Option<(String, Vec<String>)> {
    let rest = line.strip_prefix("pub").map_or(line, |after| {
        let after = after.trim_start();
        match after.strip_prefix('(') {
            Some(scoped) => scoped.split_once(')').map_or(after, |(_, rest)| rest),
            None => after,
        }
    });
    let name = rest
        .trim_start()
        .strip_prefix("mod ")?
        .trim()
        .strip_suffix(';')?
        .trim();
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let file = path.rsplit('/').next().unwrap_or(path);
    let parent = match file {
        "mod.rs" | "lib.rs" | "main.rs" | "build.rs" => folder(path).to_owned(),
        _ if is_crate_root(path, tree) => folder(path).to_owned(),
        _ => path.trim_end_matches(".rs").to_owned(),
    };
    Some((
        name.to_owned(),
        vec![
            joined(&parent, &format!("{name}.rs")),
            joined(&parent, &format!("{name}/mod.rs")),
        ],
    ))
}

/// The relative module a line imports, and the files that could be it.
fn relative_import(path: &str, line: &str) -> Option<(String, Vec<String>)> {
    if !(line.starts_with("import") || line.starts_with("export")) {
        return None;
    }
    let quoted = ["from ", "import ", "import("]
        .iter()
        .filter_map(|word| line.find(word).map(|at| at + word.len()))
        .filter(|at| line[*at..].starts_with(['"', '\'']))
        .min()?;
    let quote = &line[quoted..=quoted];
    let spec = line[quoted + 1..].split(quote).next()?;
    if !(spec.starts_with("./") || spec.starts_with("../")) || spec.ends_with('/') {
        return None;
    }
    let spec = spec.split(['?', '#']).next().unwrap_or(spec);
    let mut parts: Vec<&str> = folder(path)
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    for part in spec.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    let target = parts.join("/");
    let stem = [".js", ".jsx", ".mjs"]
        .iter()
        .find_map(|extension| target.strip_suffix(extension))
        .unwrap_or(&target);
    let mut files = vec![target.clone()];
    for extension in [".ts", ".tsx", ".js", ".jsx", ".mjs", ".json", ".d.ts"] {
        files.push(format!("{stem}{extension}"));
    }
    for index in ["index.ts", "index.tsx", "index.js", "index.jsx"] {
        files.push(format!("{target}/{index}"));
    }
    Some((spec.to_owned(), files))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_module_or_import_whose_file_is_not_committed_is_named() {
        let tree: HashSet<String> = ["crates/core/src/server/mod.rs", "src/api/core.ts"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let rust = missing(
            "crates/core/src/server/mod.rs",
            b"mod seen;\n",
            b"mod seen;\npub(crate) mod tasks;\n",
            &tree,
        );
        assert_eq!(
            rust,
            ["crates/core/src/server/mod.rs adds `mod tasks;`, but crates/core/src/server/tasks.rs is not in this commit"]
        );
        let script = missing(
            "src/api/core.ts",
            b"",
            b"import { onCoreChanged } from \"./coreChanged\";\nimport { core } from \"./core\";\n",
            &tree,
        );
        assert_eq!(
            script,
            ["src/api/core.ts adds an import of ./coreChanged, but src/api/coreChanged is not in this commit"]
        );
    }

    #[test]
    fn a_test_crates_module_and_an_asset_query_name_files_already_there() {
        let tree: HashSet<String> = [
            "crates/core/Cargo.toml",
            "crates/core/tests/folded/mod.rs",
            "brand/mark/mark-white.svg",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        assert!(missing("crates/core/tests/chat.rs", b"", b"mod folded;\n", &tree).is_empty());
        assert!(missing(
            "src/App.tsx",
            b"",
            b"import markUrl from \"../brand/mark/mark-white.svg?url\";\nexport const root = new URL('../../../', import.meta.url);\n",
            &tree,
        )
        .is_empty());
    }
}
