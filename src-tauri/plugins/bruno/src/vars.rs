// A request's variables and `{{name}}` interpolation. The scope is one flat map merged in
// precedence order: the session's own values, the request's, secrets, the environment, then
// each folder from the request's out to the collection.

use std::collections::BTreeMap;

use sikemux_plugin_api::bruno::{Env, KeyVal, Scope};

pub type Vars = BTreeMap<String, String>;

/// How deep a value may name another before interpolation stops following it.
const DEPTH: usize = 10;

/// The layers merged, earlier ones winning.
pub fn merge(layers: &[&Vars]) -> Vars {
    let mut merged = Vars::new();
    for layer in layers.iter().rev() {
        merged.extend(
            layer
                .iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        );
    }
    merged
}

pub fn of_rows(rows: &[KeyVal]) -> Vars {
    rows.iter()
        .filter(|row| row.enabled && !row.name.is_empty())
        .map(|row| (row.name.clone(), row.value.clone()))
        .collect()
}

/// What a request sees before its own variables and the session's: secrets, the environment,
/// then the scopes from the request's folder out to the collection (`scopes` outermost first).
pub fn inherited(env: Option<&Env>, secrets: &Vars, scopes: &[Scope]) -> Vars {
    let env = env.map(|env| of_rows(&env.vars)).unwrap_or_default();
    let layers: Vec<Vars> = scopes
        .iter()
        .rev()
        .map(|scope| of_rows(&scope.vars.req))
        .collect();
    let mut all: Vec<&Vars> = vec![secrets, &env];
    all.extend(layers.iter());
    merge(&all)
}

/// `input` with each `{{name}}` it knows filled in, following names a value holds. An unknown
/// name stays as written, except `process.env.*`, which is empty when unknown.
pub fn interpolate(input: &str, vars: &Vars) -> String {
    let mut text = input.to_owned();
    for _ in 0..=DEPTH {
        if !text.contains("{{") {
            break;
        }
        let next = fill(&text, vars);
        if next == text {
            break;
        }
        text = next;
    }
    text
}

fn fill(text: &str, vars: &Vars) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((before, after)) = rest.split_once("{{") {
        out.push_str(before);
        match after.split_once("}}").filter(|(key, _)| !key.contains('}')) {
            None => {
                out.push_str("{{");
                rest = after;
            }
            Some((key, tail)) => {
                match vars.get(key.trim()) {
                    Some(value) => out.push_str(value),
                    None if key.trim().starts_with("process.env.") => {}
                    None => {
                        out.push_str("{{");
                        out.push_str(key);
                        out.push_str("}}");
                    }
                }
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sikemux_plugin_api::bruno::Vars as ScopeVars;

    fn vars(pairs: &[(&str, &str)]) -> Vars {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    fn row(name: &str, value: &str) -> KeyVal {
        KeyVal {
            name: name.into(),
            value: value.into(),
            enabled: true,
        }
    }

    fn scope(rows: Vec<KeyVal>) -> Scope {
        Scope {
            vars: ScopeVars {
                req: rows,
                res: Vec::new(),
            },
            ..Scope::default()
        }
    }

    #[test]
    fn merges_scopes_with_earlier_layers_taking_precedence() {
        let runtime = vars(&[("token", "runtime"), ("base", "https://runtime")]);
        let env = vars(&[("token", "env"), ("other", "x")]);
        assert_eq!(
            merge(&[&runtime, &env]),
            vars(&[
                ("token", "runtime"),
                ("base", "https://runtime"),
                ("other", "x")
            ])
        );
    }

    #[test]
    fn interpolates_variables_recursively() {
        let scope = vars(&[
            ("host", "example.test"),
            ("baseUrl", "https://{{host}}"),
            ("path", "v1"),
        ]);
        assert_eq!(
            interpolate("{{baseUrl}}/{{ path }}", &scope),
            "https://example.test/v1"
        );
    }

    #[test]
    fn leaves_unknown_variables_but_blanks_unknown_process_environment_ones() {
        assert_eq!(
            interpolate("{{missing}} {{process.env.NOPE}}", &Vars::new()),
            "{{missing}} "
        );
    }

    #[test]
    fn builds_variable_scopes_in_documented_precedence_order() {
        let collection = scope(vec![
            row("baseUrl", "https://collection.test"),
            row("shared", "collection"),
        ]);
        let folder = scope(vec![row("shared", "folder"), row("folderOnly", "yes")]);
        let env = Env {
            vars: vec![row("baseUrl", "https://env.test"), row("envOnly", "yes")],
            ..Env::default()
        };
        let secrets = vars(&[("token", "secret"), ("shared", "secret")]);
        assert_eq!(
            inherited(Some(&env), &secrets, &[collection, folder]),
            vars(&[
                ("baseUrl", "https://env.test"),
                ("shared", "secret"),
                ("folderOnly", "yes"),
                ("envOnly", "yes"),
                ("token", "secret"),
            ])
        );
    }
}
