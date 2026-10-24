// The `.bru` block language: a file is a run of top-level blocks, `name { … }` holding
// key/value rows or verbatim text, and `name [ … ]` holding a list. Blocks the model knows are
// folded into it; the rest are kept as written so saving gives them back unchanged.

use sikemux_plugin_api::bruno::{
    Auth, AuthMode, Body, BodyMode, Env, HttpMethod, KeyPlacement, KeyVal, Meta, RawBlock, Request,
    RequestSettings, Scope,
};

/// The top-level blocks of `src`. A `'''` heredoc inside a `{` block is skipped whole, so
/// braces in an example body do not end it.
pub fn blocks(src: &str) -> Vec<RawBlock> {
    let chars: Vec<char> = src.chars().collect();
    let length = chars.len();
    let at = |index: usize| chars.get(index).copied();
    let mut blocks = Vec::new();
    let mut index = 0;
    while index < length {
        while at(index).is_some_and(char::is_whitespace) {
            index += 1;
        }
        if index >= length {
            break;
        }
        let start = index;
        while at(index).is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-')) {
            index += 1;
        }
        let name: String = chars.get(start..index).unwrap_or_default().iter().collect();
        if name.is_empty() {
            index += 1;
            continue;
        }
        while matches!(at(index), Some(' ' | '\t')) {
            index += 1;
        }
        let open = at(index);
        let close = match open {
            Some('{') => '}',
            Some('[') => ']',
            _ => {
                while at(index).is_some_and(|c| c != '\n') {
                    index += 1;
                }
                continue;
            }
        };
        let open = open.unwrap_or('{');
        index += 1;
        let inner = index;
        let mut depth = 1;
        let heredoc = |index: usize| {
            at(index) == Some('\'') && at(index + 1) == Some('\'') && at(index + 2) == Some('\'')
        };
        while index < length {
            if open == '{' && heredoc(index) {
                index += 3;
                while index < length && !heredoc(index) {
                    index += 1;
                }
                index += 3;
                continue;
            }
            match at(index) {
                Some(c) if c == open => depth += 1,
                Some(c) if c == close => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            index += 1;
        }
        let raw: String = chars
            .get(inner.min(length)..index.min(length))
            .unwrap_or_default()
            .iter()
            .collect();
        index += 1;
        blocks.push(RawBlock {
            name,
            delim: open.to_string(),
            raw,
        });
    }
    blocks
}

/// A verbatim block's text with its blank edges and shared leading indent taken off.
pub fn dedent(raw: &str) -> String {
    let mut lines: Vec<&str> = raw.split('\n').collect();
    while lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    let indent = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start_matches([' ', '\t']).len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|line| match line.trim().is_empty() {
            true => "",
            false => line.get(indent..).unwrap_or(line),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Key/value rows, one per line; a row starting with `~` is disabled, and a line with no `:`
/// is a name with an empty value.
pub fn key_vals(raw: &str) -> Vec<KeyVal> {
    raw.split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let (enabled, body) = match line.strip_prefix('~') {
                Some(rest) => (false, rest.trim_start()),
                None => (true, line),
            };
            match body.split_once(':') {
                None => KeyVal {
                    name: body.to_owned(),
                    value: String::new(),
                    enabled,
                },
                Some((name, value)) => KeyVal {
                    name: name.trim().to_owned(),
                    value: value.strip_prefix(' ').unwrap_or(value).to_owned(),
                    enabled,
                },
            }
        })
        .collect()
}

/// The enabled rows as a lookup, the first of each name winning.
fn dict(raw: &str) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    for row in key_vals(raw).into_iter().filter(|row| row.enabled) {
        if !rows.iter().any(|(name, _)| *name == row.name) {
            rows.push((row.name, row.value));
        }
    }
    rows
}

fn get<'a>(rows: &'a [(String, String)], key: &str) -> Option<&'a str> {
    rows.iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

fn list(raw: &str) -> Vec<String> {
    raw.split([',', '\n'])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

fn body_mode(name: &str) -> Option<BodyMode> {
    Some(match name {
        "json" => BodyMode::Json,
        "text" => BodyMode::Text,
        "xml" => BodyMode::Xml,
        "sparql" => BodyMode::Sparql,
        "graphql" => BodyMode::Graphql,
        "file" => BodyMode::File,
        "none" => BodyMode::None,
        "formUrlEncoded" | "form-urlencoded" => BodyMode::FormUrlencoded,
        "multipartForm" | "multipart-form" => BodyMode::MultipartForm,
        _ => return None,
    })
}

fn auth_mode(name: &str) -> Option<AuthMode> {
    Some(match name {
        "none" => AuthMode::None,
        "inherit" => AuthMode::Inherit,
        "bearer" => AuthMode::Bearer,
        "basic" => AuthMode::Basic,
        "apikey" => AuthMode::Apikey,
        "awsv4" => AuthMode::Awsv4,
        "digest" => AuthMode::Digest,
        "wsse" => AuthMode::Wsse,
        "oauth2" => AuthMode::Oauth2,
        _ => return None,
    })
}

fn method(name: &str) -> Option<HttpMethod> {
    Some(match name {
        "get" => HttpMethod::Get,
        "post" => HttpMethod::Post,
        "put" => HttpMethod::Put,
        "delete" => HttpMethod::Delete,
        "patch" => HttpMethod::Patch,
        "options" => HttpMethod::Options,
        "head" => HttpMethod::Head,
        _ => return None,
    })
}

pub fn method_name(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "get",
        HttpMethod::Post => "post",
        HttpMethod::Put => "put",
        HttpMethod::Delete => "delete",
        HttpMethod::Patch => "patch",
        HttpMethod::Options => "options",
        HttpMethod::Head => "head",
    }
}

fn apply_auth_detail(auth: &mut Auth, name: &str, raw: &str) {
    let rows = dict(raw);
    let value = |key: &str| get(&rows, key).unwrap_or_default().to_owned();
    match name {
        "auth:bearer" => auth.bearer.token = value("token"),
        "auth:basic" => {
            auth.basic.username = value("username");
            auth.basic.password = value("password");
        }
        "auth:apikey" => {
            auth.apikey.key = value("key");
            auth.apikey.value = value("value");
            let place = get(&rows, "placement")
                .or_else(|| get(&rows, "addTo"))
                .unwrap_or("header");
            auth.apikey.placement = match place.to_lowercase().contains("query") {
                true => KeyPlacement::Queryparams,
                false => KeyPlacement::Header,
            };
        }
        _ => {}
    }
}

fn meta(raw: &str, name: &str, kind: &str) -> Meta {
    let rows = dict(raw);
    Meta {
        name: get(&rows, "name").unwrap_or(name).to_owned(),
        kind: get(&rows, "type").unwrap_or(kind).to_owned(),
        seq: get(&rows, "seq")
            .filter(|seq| !seq.is_empty())
            .and_then(|seq| seq.trim().parse().ok()),
    }
}

pub fn request(src: &str) -> Request {
    let mut request = Request {
        meta: Meta {
            kind: "http".into(),
            ..Meta::default()
        },
        auth: Auth::default(),
        ..Request::default()
    };
    let mut body_from_method = None;
    let mut auth_from_method = None;
    let mut bodies: Vec<(BodyMode, RawBlock)> = Vec::new();
    for block in blocks(src) {
        let name = block.name.as_str();
        let raw = block.raw.as_str();
        if name == "meta" {
            request.meta = meta(raw, "", "http");
        } else if let Some(verb) = method(name) {
            request.method = verb;
            let rows = dict(raw);
            request.url = get(&rows, "url").unwrap_or_default().to_owned();
            if let Some(body) = get(&rows, "body") {
                body_from_method = body_mode(body);
            }
            if let Some(auth) = get(&rows, "auth") {
                auth_from_method = auth_mode(auth);
            }
        } else if name == "headers" {
            request.headers = key_vals(raw);
        } else if name == "params:query" {
            request.params.query = key_vals(raw);
        } else if name == "params:path" {
            request.params.path = key_vals(raw);
        } else if let Some(mode) = name.strip_prefix("body:").and_then(body_mode) {
            bodies.push((mode, block));
        } else if name == "auth" {
            if let Some(mode) = get(&dict(raw), "mode").and_then(auth_mode) {
                auth_from_method = auth_from_method.or(Some(mode));
            }
        } else if matches!(name, "auth:bearer" | "auth:basic" | "auth:apikey") {
            apply_auth_detail(&mut request.auth, name, raw);
        } else if matches!(name, "vars:pre-request" | "vars") {
            request.vars.req = key_vals(raw);
        } else if name == "vars:post-response" {
            request.vars.res = key_vals(raw);
        } else if name == "assert" {
            request.assertions = key_vals(raw);
        } else if name == "script:pre-request" {
            request.scripts.pre = dedent(raw);
        } else if name == "script:post-response" {
            request.scripts.post = dedent(raw);
        } else if name == "tests" {
            request.tests = dedent(raw);
        } else if name == "docs" {
            request.docs = dedent(raw);
        } else if name == "settings" {
            let rows = dict(raw);
            request.settings = RequestSettings {
                encode_url: get(&rows, "encodeUrl").map(|value| value == "true"),
                timeout: get(&rows, "timeout").and_then(|value| value.trim().parse().ok()),
            };
        } else {
            request.extra.push(block);
        }
    }
    // The method block's `body:` names the body in use; content of another kind is kept as
    // written so it survives a save.
    request.body.mode = body_from_method
        .or(bodies.first().map(|(mode, _)| *mode))
        .unwrap_or(BodyMode::None);
    let mut consumed = false;
    for (mode, block) in bodies {
        if !consumed && mode == request.body.mode {
            consumed = true;
            match mode {
                BodyMode::FormUrlencoded | BodyMode::MultipartForm | BodyMode::File => {
                    request.body.form = key_vals(&block.raw);
                }
                _ => request.body.text = dedent(&block.raw),
            }
        } else {
            request.extra.push(block);
        }
    }
    request.auth.mode = auth_from_method.unwrap_or(AuthMode::Inherit);
    request
}

/// A collection's or folder's shared blocks, named `name` when its meta names none.
pub fn scope(src: &str, name: &str) -> Scope {
    let mut scope = Scope {
        meta: Meta {
            name: name.to_owned(),
            ..Meta::default()
        },
        ..Scope::default()
    };
    for block in blocks(src) {
        let raw = block.raw.as_str();
        match block.name.as_str() {
            "meta" => scope.meta = meta(raw, name, ""),
            "auth" => {
                if let Some(mode) = get(&dict(raw), "mode").and_then(auth_mode) {
                    scope.auth.mode = mode;
                }
            }
            detail @ ("auth:bearer" | "auth:basic" | "auth:apikey") => {
                apply_auth_detail(&mut scope.auth, detail, raw)
            }
            "headers" => scope.headers = key_vals(raw),
            "vars" | "vars:pre-request" => scope.vars.req = key_vals(raw),
            "vars:post-response" => scope.vars.res = key_vals(raw),
            "script:pre-request" => scope.scripts.pre = dedent(raw),
            "script:post-response" => scope.scripts.post = dedent(raw),
            _ => scope.extra.push(block),
        }
    }
    scope
}

pub fn env(src: &str, name: &str, collection_path: &str, collection_name: &str) -> Env {
    let mut env = Env {
        id: format!("{collection_path}::{name}"),
        name: name.to_owned(),
        collection_path: collection_path.to_owned(),
        collection_name: collection_name.to_owned(),
        ..Env::default()
    };
    for block in blocks(src) {
        match block.name.as_str() {
            "vars" => env.vars = key_vals(&block.raw),
            "vars:secret" => env.secret_names = list(&block.raw),
            _ => {}
        }
    }
    env
}

fn indented(text: &str) -> String {
    text.split('\n')
        .map(|line| match line.is_empty() {
            true => String::new(),
            false => format!("  {line}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn row(row: &KeyVal) -> String {
    format!(
        "  {}{}: {}",
        if row.enabled { "" } else { "~" },
        row.name,
        row.value
    )
}

fn dict_block(name: &str, rows: &[KeyVal]) -> Option<String> {
    (!rows.is_empty()).then(|| {
        let lines: Vec<String> = rows.iter().map(row).collect();
        format!("{name} {{\n{}\n}}", lines.join("\n"))
    })
}

fn text_block(name: &str, text: &str) -> Option<String> {
    (!text.is_empty()).then(|| format!("{name} {{\n{}\n}}", indented(text)))
}

fn raw_block(block: &RawBlock) -> String {
    let close = if block.delim == "{" { "}" } else { "]" };
    format!("{} {}{}{close}", block.name, block.delim, block.raw)
}

fn body_ref(mode: BodyMode) -> &'static str {
    match mode {
        BodyMode::None => "none",
        BodyMode::Json => "json",
        BodyMode::Text => "text",
        BodyMode::Xml => "xml",
        BodyMode::Sparql => "sparql",
        BodyMode::Graphql => "graphql",
        BodyMode::FormUrlencoded => "formUrlEncoded",
        BodyMode::MultipartForm => "multipartForm",
        BodyMode::File => "file",
    }
}

fn auth_name(mode: AuthMode) -> &'static str {
    match mode {
        AuthMode::None => "none",
        AuthMode::Inherit => "inherit",
        AuthMode::Bearer => "bearer",
        AuthMode::Basic => "basic",
        AuthMode::Apikey => "apikey",
        AuthMode::Awsv4 => "awsv4",
        AuthMode::Digest => "digest",
        AuthMode::Wsse => "wsse",
        AuthMode::Oauth2 => "oauth2",
    }
}

fn meta_block(meta: &Meta) -> String {
    let mut lines = vec![format!("  name: {}", meta.name)];
    if !meta.kind.is_empty() {
        lines.push(format!("  type: {}", meta.kind));
    }
    if let Some(seq) = meta.seq {
        lines.push(format!("  seq: {seq}"));
    }
    format!("meta {{\n{}\n}}", lines.join("\n"))
}

fn auth_detail(auth: &Auth) -> Option<String> {
    match auth.mode {
        AuthMode::Bearer => Some(format!(
            "auth:bearer {{\n  token: {}\n}}",
            auth.bearer.token
        )),
        AuthMode::Basic => Some(format!(
            "auth:basic {{\n  username: {}\n  password: {}\n}}",
            auth.basic.username, auth.basic.password
        )),
        AuthMode::Apikey => Some(format!(
            "auth:apikey {{\n  key: {}\n  value: {}\n  placement: {}\n}}",
            auth.apikey.key,
            auth.apikey.value,
            match auth.apikey.placement {
                KeyPlacement::Queryparams => "queryparams",
                KeyPlacement::Header => "header",
            }
        )),
        _ => None,
    }
}

fn body_block(body: &Body) -> Option<String> {
    match body.mode {
        BodyMode::None => None,
        BodyMode::Json | BodyMode::Text | BodyMode::Xml | BodyMode::Sparql | BodyMode::Graphql => {
            text_block(&format!("body:{}", body_ref(body.mode)), &body.text)
        }
        BodyMode::FormUrlencoded => dict_block("body:form-urlencoded", &body.form),
        BodyMode::MultipartForm => dict_block("body:multipart-form", &body.form),
        BodyMode::File => dict_block("body:file", &body.form),
    }
}

fn joined(parts: Vec<Option<String>>) -> String {
    let parts: Vec<String> = parts.into_iter().flatten().collect();
    format!("{}\n", parts.join("\n\n"))
}

/// The request as a `.bru` file, its blocks in Bruno's own order.
pub fn write_request(request: &Request) -> String {
    let mut parts = vec![
        Some(meta_block(&request.meta)),
        Some(format!(
            "{} {{\n  url: {}\n  body: {}\n  auth: {}\n}}",
            method_name(request.method),
            request.url,
            body_ref(request.body.mode),
            auth_name(request.auth.mode)
        )),
        dict_block("params:query", &request.params.query),
        dict_block("params:path", &request.params.path),
        dict_block("headers", &request.headers),
        auth_detail(&request.auth),
        body_block(&request.body),
        dict_block("vars:pre-request", &request.vars.req),
        dict_block("vars:post-response", &request.vars.res),
        dict_block("assert", &request.assertions),
        text_block("script:pre-request", &request.scripts.pre),
        text_block("script:post-response", &request.scripts.post),
        text_block("tests", &request.tests),
        text_block("docs", &request.docs),
    ];
    let mut settings = Vec::new();
    if let Some(encode) = request.settings.encode_url {
        settings.push(format!("  encodeUrl: {encode}"));
    }
    if let Some(timeout) = request.settings.timeout {
        settings.push(format!("  timeout: {timeout}"));
    }
    if !settings.is_empty() {
        parts.push(Some(format!("settings {{\n{}\n}}", settings.join("\n"))));
    }
    parts.extend(request.extra.iter().map(|block| Some(raw_block(block))));
    joined(parts)
}

/// A new request named `name`: a GET with no URL that takes its auth from above.
pub fn new_request(name: &str) -> Request {
    Request {
        meta: Meta {
            name: name.to_owned(),
            kind: "http".into(),
            seq: None,
        },
        settings: RequestSettings {
            encode_url: Some(true),
            timeout: Some(0),
        },
        ..Request::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUEST: &str = r#"
meta {
  name: Get User
  type: http
  seq: 3
}

post {
  url: {{baseUrl}}/users
  body: json
  auth: bearer
}

params:query {
  enabled: true
  ~disabled: no
}

headers {
  Content-Type: application/json
}

auth:bearer {
  token: {{token}}
}

body:json {
  {
    "name": "Ada"
  }
}

script:pre-request {
  console.log("hi")
}

example {
  keep: me
}
"#;

    fn kv(name: &str, value: &str, enabled: bool) -> KeyVal {
        KeyVal {
            name: name.into(),
            value: value.into(),
            enabled,
        }
    }

    #[test]
    fn lexes_top_level_blocks_and_ignores_braces_inside_heredocs() {
        let found = blocks("script {\n  '''\n  { not a block }\n  '''\n}\nmeta {\n  name: ok\n}\n");
        let names: Vec<&str> = found.iter().map(|block| block.name.as_str()).collect();
        assert_eq!(names, ["script", "meta"]);
    }

    #[test]
    fn dedents_verbatim_block_content() {
        assert_eq!(
            dedent("\n    line one\n      line two\n"),
            "line one\n  line two"
        );
    }

    #[test]
    fn parses_enabled_and_disabled_rows() {
        assert_eq!(
            key_vals("a: 1\n~b: 2\nflag\n"),
            [
                kv("a", "1", true),
                kv("b", "2", false),
                kv("flag", "", true)
            ]
        );
    }

    #[test]
    fn parses_a_request_into_the_model_and_keeps_unknown_blocks() {
        let parsed = request(REQUEST);
        assert_eq!(
            parsed.meta,
            Meta {
                name: "Get User".into(),
                kind: "http".into(),
                seq: Some(3)
            }
        );
        assert_eq!(parsed.method, HttpMethod::Post);
        assert_eq!(parsed.url, "{{baseUrl}}/users");
        assert_eq!(parsed.body.mode, BodyMode::Json);
        assert!(parsed.body.text.contains("\"name\": \"Ada\""));
        assert_eq!(parsed.auth.mode, AuthMode::Bearer);
        assert_eq!(parsed.auth.bearer.token, "{{token}}");
        assert!(parsed.params.query.contains(&kv("disabled", "no", false)));
        assert_eq!(
            parsed.headers,
            [kv("Content-Type", "application/json", true)]
        );
        assert_eq!(parsed.scripts.pre, "console.log(\"hi\")");
        let extra: Vec<&str> = parsed
            .extra
            .iter()
            .map(|block| block.name.as_str())
            .collect();
        assert_eq!(extra, ["example"]);
    }

    #[test]
    fn parses_scopes_and_environments() {
        let folder = scope(
            "meta {\n  name: folder\n}\nvars {\n  baseUrl: https://example.test\n}\n",
            "fallback",
        );
        assert_eq!(folder.meta.name, "folder");
        assert_eq!(
            folder.vars.req,
            [kv("baseUrl", "https://example.test", true)]
        );
        let dev = env(
            "vars {\n  token: abc\n}\nvars:secret [\n  password,\n  otp\n]\n",
            "dev",
            "/api",
            "API",
        );
        assert_eq!(dev.id, "/api::dev");
        assert_eq!(dev.vars, [kv("token", "abc", true)]);
        assert_eq!(dev.secret_names, ["password", "otp"]);
    }

    #[test]
    fn writes_a_stable_minimal_request() {
        let mut ping = new_request("Ping");
        ping.url = "{{baseUrl}}/ping".into();
        ping.headers = vec![kv("Accept", "application/json", true)];
        let text = write_request(&ping);
        assert!(text.contains("meta {\n  name: Ping\n  type: http\n}"));
        assert!(text.contains("get {\n  url: {{baseUrl}}/ping\n  body: none\n  auth: inherit\n}"));
        assert!(text.contains("headers {\n  Accept: application/json\n}"));
    }

    #[test]
    fn round_trips_a_parsed_request() {
        let src = "meta {\n  name: Create\n  type: http\n}\n\npost {\n  url: /users\n  body: json\n  auth: basic\n}\n\nauth:basic {\n  username: ada\n  password: secret\n}\n\nbody:json {\n  {\"ok\":true}\n}\n";
        let parsed = request(src);
        assert_eq!(request(&write_request(&parsed)), parsed);
        assert_eq!(request(&write_request(&request(REQUEST))), request(REQUEST));
    }
}
