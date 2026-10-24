// Running a request as Bruno would: its variables filled in, its auth taken from the nearest
// scope that sets one, its headers, query and body built, the collection's and its own
// scripts run around the call, then its response variables and assertions.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use base64::Engine as _;
use serde::Deserialize;
use serde_json::Value;
use sikemux_plugin_api::bruno::{
    Auth, AuthMode, BodyMode, Check, Env, Header, KeyPlacement, KeyVal, Request, Response,
    RunResult, Scope, ScriptLog, Sent,
};

use crate::bru;
use crate::collection::{self, RequestAsk};
use crate::error::BrunoResult;
use crate::scripts::{self, Ctx, ReqView, ResView};
use crate::send::{self, BruBodyWire, BruSendRequest, BruTrust, MultipartField};
use crate::vars::{self, interpolate, Vars};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunAsk {
    pub root: String,
    pub path: String,
    /// The request as edited, when it has changes not saved yet.
    #[serde(default)]
    pub request: Option<Request>,
    /// The environment picked, by its id.
    #[serde(default)]
    pub env: Option<String>,
    /// The secret values typed in this session.
    #[serde(default)]
    pub secrets: Vars,
    /// The values earlier runs' scripts set in this session.
    #[serde(default)]
    pub runtime: Vars,
    /// The person trusted this collection for the session: its scripts may run, and it may
    /// reach private addresses and read files inside its folder.
    #[serde(default)]
    pub trusted: bool,
}

fn enabled(rows: &[KeyVal]) -> impl Iterator<Item = &KeyVal> {
    rows.iter()
        .filter(|row| row.enabled && !row.name.is_empty())
}

/// The request's own auth, or else the nearest scope's that sets one, folders before the
/// collection.
pub fn effective_auth<'a>(request: &'a Request, scopes: &'a [Scope]) -> &'a Auth {
    if request.auth.mode != AuthMode::Inherit {
        return &request.auth;
    }
    scopes
        .iter()
        .rev()
        .map(|scope| &scope.auth)
        .find(|auth| auth.mode != AuthMode::Inherit)
        .unwrap_or(&request.auth)
}

fn apply_auth(
    auth: &Auth,
    headers: &mut Vec<(String, String)>,
    query: &mut Vec<(String, String)>,
    vars: &Vars,
) {
    let fill = |text: &str| interpolate(text, vars);
    match auth.mode {
        AuthMode::Bearer if !auth.bearer.token.is_empty() => {
            headers.push((
                "Authorization".into(),
                format!("Bearer {}", fill(&auth.bearer.token)),
            ));
        }
        AuthMode::Basic => {
            let pair = format!(
                "{}:{}",
                fill(&auth.basic.username),
                fill(&auth.basic.password)
            );
            let encoded = base64::engine::general_purpose::STANDARD.encode(pair);
            headers.push(("Authorization".into(), format!("Basic {encoded}")));
        }
        AuthMode::Apikey if !auth.apikey.key.is_empty() => {
            let pair = (fill(&auth.apikey.key), fill(&auth.apikey.value));
            match auth.apikey.placement {
                KeyPlacement::Queryparams => query.push(pair),
                KeyPlacement::Header => headers.push(pair),
            }
        }
        _ => {}
    }
}

/// The body as a script's `req.getBody()` gives it: an object for JSON, text otherwise.
fn body_for_script(request: &Request, vars: &Vars) -> Value {
    let body = &request.body;
    match body.mode {
        BodyMode::Json | BodyMode::Graphql => {
            let text = interpolate(&body.text, vars);
            serde_json::from_str(&text).unwrap_or(Value::String(text))
        }
        BodyMode::Text | BodyMode::Xml | BodyMode::Sparql => {
            Value::String(interpolate(&body.text, vars))
        }
        BodyMode::FormUrlencoded | BodyMode::MultipartForm | BodyMode::File => Value::Object(
            enabled(&body.form)
                .map(|row| (row.name.clone(), Value::String(row.value.clone())))
                .collect(),
        ),
        BodyMode::None => Value::Null,
    }
}

fn content_type(mode: BodyMode) -> &'static str {
    match mode {
        BodyMode::Json | BodyMode::Graphql => "application/json",
        BodyMode::Xml => "application/xml",
        BodyMode::Sparql => "application/sparql-query",
        _ => "text/plain",
    }
}

fn body_wire(request: &Request, script_body: &Value, vars: &Vars) -> BruBodyWire {
    let fill = |text: &str| interpolate(text, vars);
    match request.body.mode {
        BodyMode::None => BruBodyWire::None,
        BodyMode::File => match enabled(&request.body.form).next() {
            Some(row) => {
                let path = fill(if row.value.is_empty() {
                    &row.name
                } else {
                    &row.value
                });
                match path.is_empty() {
                    true => BruBodyWire::None,
                    false => BruBodyWire::File {
                        path,
                        content_type: None,
                    },
                }
            }
            None => BruBodyWire::None,
        },
        BodyMode::FormUrlencoded => BruBodyWire::Form {
            fields: enabled(&request.body.form)
                .map(|row| (fill(&row.name), fill(&row.value)))
                .collect(),
        },
        BodyMode::MultipartForm => BruBodyWire::Multipart {
            fields: enabled(&request.body.form)
                .map(|row| {
                    let value = row.value.trim();
                    match value
                        .strip_prefix("@file(")
                        .and_then(|rest| rest.strip_suffix(')'))
                    {
                        Some(path) => MultipartField {
                            name: fill(&row.name),
                            value: fill(path),
                            is_file: true,
                        },
                        None => MultipartField {
                            name: fill(&row.name),
                            value: fill(&row.value),
                            is_file: false,
                        },
                    }
                })
                .collect(),
        },
        mode => {
            // A pre-request script may have replaced the body.
            let data = match script_body {
                Value::String(text) => text.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            };
            BruBodyWire::Raw {
                content_type: Some(content_type(mode).into()),
                data,
            }
        }
    }
}

/// `text` percent-encoded as JavaScript's `encodeURIComponent` does it.
fn encoded(text: &str, encode: bool) -> String {
    if !encode {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(char::from(byte)),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `url` with each `:name` ending a path part, and each `{name}`, filled in.
fn with_path_params(url: &str, params: &[(String, String)], encode: bool) -> String {
    let mut out = url.to_owned();
    for (name, value) in params.iter().filter(|(name, _)| !name.is_empty()) {
        let value = encoded(value, encode);
        out = out.replace(&format!("{{{name}}}"), &value);
        let marker = format!(":{name}");
        let mut filled = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(at) = rest.find(&marker) {
            let after = rest.get(at + marker.len()..).unwrap_or_default();
            let ends = after.is_empty() || after.starts_with(['/', '?', '#']);
            filled.push_str(rest.get(..at).unwrap_or_default());
            filled.push_str(if ends { &value } else { &marker });
            rest = after;
        }
        filled.push_str(rest);
        out = filled;
    }
    out
}

fn with_query(url: &str, query: &[(String, String)], encode: bool) -> String {
    if query.is_empty() {
        return url.to_owned();
    }
    let (before, hash) = match url.find('#') {
        Some(at) => url.split_at(at),
        None => (url, ""),
    };
    let pairs: Vec<String> = query
        .iter()
        .map(|(name, value)| format!("{}={}", encoded(name, encode), encoded(value, encode)))
        .collect();
    let joiner = match before.contains('?') {
        true if before.ends_with(['?', '&']) => "",
        true => "&",
        false => "?",
    };
    format!("{before}{joiner}{}{hash}", pairs.join("&"))
}

/// An expression's value as a variable holds it.
fn as_variable(value: Option<Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text,
        Some(other) => other.to_string(),
    }
}

/// The environment `id` names, `<collection>::<name>`, read from its file.
fn env(root: &str, id: &str) -> BrunoResult<Option<Env>> {
    let Some((collection, name)) = id.rsplit_once("::") else {
        return Ok(None);
    };
    let path = Path::new(collection)
        .join("environments")
        .join(format!("{name}.bru"));
    let text = path.display().to_string();
    collection::check_inside(root, &text)?;
    let owner = Path::new(collection)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(Some(bru::env(
        &fs::read_to_string(&path)?,
        name,
        collection,
        &owner,
    )))
}

const ASSERT_OPS: [&str; 28] = [
    "eq",
    "neq",
    "gt",
    "gte",
    "lt",
    "lte",
    "in",
    "notIn",
    "contains",
    "notContains",
    "length",
    "matches",
    "notMatches",
    "startsWith",
    "endsWith",
    "between",
    "isEmpty",
    "notEmpty",
    "isNull",
    "isUndefined",
    "isDefined",
    "isTruthy",
    "isFalsy",
    "isJson",
    "isNumber",
    "isString",
    "isBoolean",
    "isArray",
];

pub async fn run(ask: RunAsk) -> BrunoResult<RunResult> {
    let view = {
        let asked = RequestAsk {
            root: ask.root.clone(),
            path: ask.path.clone(),
        };
        let root = ask.root.clone();
        let picked = ask.env.clone();
        tokio::task::spawn_blocking(move || -> BrunoResult<_> {
            let view = collection::request(&asked)?;
            let env = match picked {
                Some(id) => env(&root, &id)?,
                None => None,
            };
            Ok((view, env))
        })
        .await
        .map_err(|error| crate::error::BrunoError::Http(error.to_string()))??
    };
    let (view, env) = view;
    let request = ask.request.unwrap_or(view.request);
    let scopes = view.scopes;
    let inherited = vars::inherited(env.as_ref(), &ask.secrets, &scopes);
    let own = vars::of_rows(&request.vars.req);
    let start = vars::merge(&[&ask.runtime, &own, &inherited]);
    let trust = BruTrust {
        allow_private_network: ask.trusted,
        allow_file_read: ask.trusted,
        allow_insecure_tls: ask.trusted,
        file_root: Some(ask.root.clone()),
    };

    let mut headers = BTreeMap::new();
    for row in scopes
        .iter()
        .flat_map(|scope| enabled(&scope.headers))
        .chain(enabled(&request.headers))
    {
        headers.insert(
            interpolate(&row.name, &start),
            interpolate(&row.value, &start),
        );
    }
    let mut ctx = Ctx {
        req: ReqView {
            method: bru::method_name(request.method).to_uppercase(),
            url: interpolate(&request.url, &start),
            headers,
            body: body_for_script(&request, &start),
            name: request.meta.name.clone(),
        },
        vars: start.clone(),
        res: None,
        logs: Vec::new(),
        tests: Vec::new(),
        trust: trust.clone(),
    };
    if ask.trusted {
        for code in scopes
            .iter()
            .map(|scope| &scope.scripts.pre)
            .chain([&request.scripts.pre])
        {
            scripts::run_script(code, &mut ctx).await;
        }
    }

    let vars = ctx.vars.clone();
    let mut sent_headers: Vec<(String, String)> = ctx
        .req
        .headers
        .iter()
        .map(|(name, value)| (name.clone(), interpolate(value, &vars)))
        .collect();
    let encode = request.settings.encode_url != Some(false);
    let path_params: Vec<(String, String)> = enabled(&request.params.path)
        .map(|row| {
            (
                interpolate(&row.name, &vars),
                interpolate(&row.value, &vars),
            )
        })
        .collect();
    let mut query: Vec<(String, String)> = enabled(&request.params.query)
        .map(|row| {
            (
                interpolate(&row.name, &vars),
                interpolate(&row.value, &vars),
            )
        })
        .collect();
    apply_auth(
        effective_auth(&request, &scopes),
        &mut sent_headers,
        &mut query,
        &vars,
    );
    let url = with_query(
        &with_path_params(&interpolate(&ctx.req.url, &vars), &path_params, encode),
        &query,
        encode,
    );
    let sent = Sent {
        method: ctx.req.method.clone(),
        url: url.clone(),
        headers: sent_headers
            .iter()
            .map(|(name, value)| Header {
                name: name.clone(),
                value: value.clone(),
            })
            .collect(),
    };
    let wire = BruSendRequest {
        method: ctx.req.method.clone(),
        url,
        headers: sent_headers,
        body: body_wire(&request, &ctx.req.body, &vars),
        timeout_ms: request
            .settings
            .timeout
            .filter(|timeout| *timeout > 0)
            .unwrap_or(0),
        skip_tls_verify: false,
        trust,
    };
    let changed = |ctx: &Ctx| -> Vars {
        ctx.vars
            .iter()
            .filter(|(name, value)| start.get(*name) != Some(*value))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    };

    let response = match send::send(wire).await {
        Ok(response) => response,
        Err(error) => {
            return Ok(RunResult {
                response: None,
                error: Some(error.to_string()),
                sent,
                env_updates: changed(&ctx),
                logs: ctx.logs,
                tests: ctx.tests,
            });
        }
    };

    if ask.trusted {
        ctx.res = Some(ResView {
            status: response.status,
            status_text: response.status_text.clone(),
            headers: response.headers.iter().cloned().collect(),
            body: serde_json::from_str(&response.body)
                .unwrap_or_else(|_| Value::String(response.body.clone())),
            response_time: response.duration_ms,
        });
        for code in scopes
            .iter()
            .map(|scope| &scope.scripts.post)
            .chain([&request.scripts.post])
        {
            scripts::run_script(code, &mut ctx).await;
        }
        for row in scopes
            .iter()
            .flat_map(|scope| enabled(&scope.vars.res))
            .chain(enabled(&request.vars.res))
        {
            if row.value.trim().is_empty() {
                ctx.vars.insert(row.name.clone(), String::new());
                continue;
            }
            match scripts::expression(&row.value, ctx.res.as_ref(), &ctx.req, &ctx.vars).await {
                Ok(value) => {
                    ctx.vars.insert(row.name.clone(), as_variable(value));
                }
                Err(error) => ctx.logs.push(ScriptLog {
                    level: "error".into(),
                    text: format!("post-response var {}: {error}", row.name),
                }),
            }
        }
        for row in request
            .assertions
            .iter()
            .filter(|row| row.enabled && !row.name.trim().is_empty())
        {
            let raw = row.value.trim();
            let mut words = raw.split_whitespace();
            let first = words.next().unwrap_or_default();
            let (op, expected) = match ASSERT_OPS.contains(&first) {
                true => (first, words.collect::<Vec<_>>().join(" ")),
                false => ("eq", raw.to_owned()),
            };
            let name = format!("{} {raw}", row.name);
            let check = match scripts::assertion(
                &row.name,
                op,
                &expected,
                ctx.res.as_ref(),
                &ctx.req,
                &ctx.vars,
            )
            .await
            {
                Ok((passed, error)) => Check {
                    name,
                    passed,
                    error,
                },
                Err(error) => Check {
                    name,
                    passed: false,
                    error: Some(format!("eval error: {error}")),
                },
            };
            ctx.tests.push(check);
        }
    }

    Ok(RunResult {
        response: Some(Response {
            status: response.status,
            status_text: response.status_text,
            headers: response
                .headers
                .into_iter()
                .map(|(name, value)| Header { name, value })
                .collect(),
            body: response.body,
            is_binary: response.is_binary,
            size_bytes: response.size_bytes,
            duration_ms: response.duration_ms,
        }),
        error: None,
        sent,
        env_updates: changed(&ctx),
        logs: ctx.logs,
        tests: ctx.tests,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sikemux_plugin_api::bruno::{BasicAuth, BearerAuth};

    fn auth(mode: AuthMode) -> Auth {
        Auth {
            mode,
            ..Auth::default()
        }
    }

    #[test]
    fn takes_auth_from_the_request_then_the_nearest_scope_that_sets_one() {
        let mut request = Request::default();
        let collection = Scope {
            auth: Auth {
                bearer: BearerAuth {
                    token: "collection".into(),
                },
                ..auth(AuthMode::Bearer)
            },
            ..Scope::default()
        };
        let root = Scope {
            auth: Auth {
                basic: BasicAuth {
                    username: "root".into(),
                    password: "pw".into(),
                },
                ..auth(AuthMode::Basic)
            },
            ..Scope::default()
        };
        let leaf = Scope::default();
        let scopes = [collection, root.clone(), leaf];
        assert_eq!(effective_auth(&request, &scopes), &root.auth);
        request.auth = Auth {
            bearer: BearerAuth {
                token: "request".into(),
            },
            ..auth(AuthMode::Bearer)
        };
        assert_eq!(effective_auth(&request, &scopes), &request.auth);
    }
}
