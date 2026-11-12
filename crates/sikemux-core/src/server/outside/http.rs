//! The MCP endpoint outside programs connect to, on this computer only: Streamable HTTP for
//! the tools, and the OAuth endpoints a client signs in through.

use crate::server::parts::Parts;
use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::{Arc, Weak};
use std::time::Duration;

use hyper::body::Incoming;
use hyper::header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE, HOST, LOCATION, ORIGIN};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream};
use url::Url;

use super::oauth::{authorize_query, Refusal};
use super::{changed, chats, Caller, ANSWER_WITHIN};
use crate::server::Core;

/// Fixed, so the address an app was given keeps working. A dev build has its own.
const PORT: u16 = if cfg!(debug_assertions) { 47732 } else { 47731 };
const PORT_VAR: &str = "SIKEMUX_MCP_PORT";
const RETRY: Duration = Duration::from_secs(10);
const MAX_BODY: usize = 1024 * 1024;
const PROTOCOLS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const INSTRUCTIONS: &str = "Sikemux runs coding agents in chats on this computer. chat_list shows its projects, agents and chats; chat_start starts a chat with a first message; chat_wait waits for its turn to end; chat_read reads what it said; chat_send sends it more. Chats you start show in Sikemux beside the person's own.";

type Body = reqwest::Body;

fn port() -> Option<u16> {
    match std::env::var(PORT_VAR) {
        Ok(value) => value.parse().ok().filter(|port| *port != 0),
        Err(_) => Some(PORT),
    }
}

/// Listens until the core stops, trying again while another core still holds the port, as
/// during an update.
pub(crate) async fn serve(core: Weak<Core>) {
    let Some(port) = port() else {
        return;
    };
    let listener = loop {
        match TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).await {
            Ok(listener) => break listener,
            Err(error) => {
                let Some(core) = core.upgrade() else {
                    return;
                };
                core.outside().set_listening(
                    None,
                    Some(format!("port {port} on this computer is taken: {error}")),
                );
                drop(core);
                tokio::time::sleep(RETRY).await;
            }
        }
    };
    {
        let Some(core) = core.upgrade() else {
            return;
        };
        core.outside()
            .set_listening(Some(format!("http://127.0.0.1:{port}/mcp")), None);
        changed(&core);
    }
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        if core.strong_count() == 0 {
            return;
        }
        tokio::spawn(connection(stream, core.clone(), port));
    }
}

async fn connection(stream: TcpStream, core: Weak<Core>, port: u16) {
    let _ = stream.set_nodelay(true);
    let service = service_fn(move |request| {
        let core = core.clone();
        async move { Ok::<_, Infallible>(answer(core, port, request).await) }
    });
    let _ = http1::Builder::new()
        .serve_connection(TokioIo::new(stream), service)
        .await;
}

fn reply(status: StatusCode, content_type: &str, body: impl Into<Body>) -> Response<Body> {
    let mut response = Response::new(body.into());
    *response.status_mut() = status;
    if let Ok(value) = HeaderValue::from_str(content_type) {
        response.headers_mut().insert(CONTENT_TYPE, value);
    }
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
}

fn json_reply(status: StatusCode, body: &Value) -> Response<Body> {
    reply(status, "application/json", body.to_string())
}

fn see_other(to: &str) -> Response<Body> {
    let mut response = reply(StatusCode::SEE_OTHER, "text/plain", "");
    if let Ok(value) = HeaderValue::from_str(to) {
        response.headers_mut().insert(LOCATION, value);
    }
    response
}

fn oauth_refusal(status: StatusCode, refusal: &Refusal) -> Response<Body> {
    json_reply(status, &refusal.body())
}

/// The request names this computer, so a page on another site cannot reach the endpoint by
/// rebinding its own name to 127.0.0.1.
fn addressed_here(request: &Request<Incoming>, port: u16) -> bool {
    let host = request
        .headers()
        .get(HOST)
        .and_then(|host| host.to_str().ok())
        .unwrap_or_default();
    [
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        format!("[::1]:{port}"),
    ]
    .iter()
    .any(|here| host.eq_ignore_ascii_case(here))
}

/// No web page sent it: a browser names the page's origin, and a page here would be served
/// from this computer.
fn not_from_a_page(request: &Request<Incoming>) -> bool {
    let Some(origin) = request.headers().get(ORIGIN) else {
        return true;
    };
    let Some(url) = origin
        .to_str()
        .ok()
        .and_then(|origin| Url::parse(origin).ok())
    else {
        return false;
    };
    matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
}

async fn read_body(mut body: Incoming) -> Option<Vec<u8>> {
    use hyper::body::Body as _;
    let mut bytes = Vec::new();
    while let Some(frame) =
        std::future::poll_fn(|cx| std::pin::Pin::new(&mut body).poll_frame(cx)).await
    {
        if let Ok(data) = frame.ok()?.into_data() {
            bytes.extend_from_slice(&data);
            if bytes.len() > MAX_BODY {
                return None;
            }
        }
    }
    Some(bytes)
}

fn pairs(text: &str) -> Vec<(String, String)> {
    url::form_urlencoded::parse(text.as_bytes())
        .into_owned()
        .collect()
}

async fn answer(core: Weak<Core>, port: u16, request: Request<Incoming>) -> Response<Body> {
    if !addressed_here(&request, port) {
        return reply(
            StatusCode::MISDIRECTED_REQUEST,
            "text/plain",
            "Sikemux answers only to 127.0.0.1 and localhost",
        );
    }
    let Some(core) = core.upgrade() else {
        return reply(
            StatusCode::SERVICE_UNAVAILABLE,
            "text/plain",
            "Sikemux is stopping",
        );
    };
    let base = format!("http://127.0.0.1:{port}");
    let path = request.uri().path().to_owned();
    let query = pairs(request.uri().query().unwrap_or_default());
    match (request.method().clone(), path.as_str()) {
        (
            Method::GET,
            "/.well-known/oauth-protected-resource" | "/.well-known/oauth-protected-resource/mcp",
        ) => json_reply(
            StatusCode::OK,
            &json!({
                "resource": format!("{base}/mcp"),
                "authorization_servers": [base],
                "scopes_supported": ["chats:read", "chats:write"],
                "bearer_methods_supported": ["header"],
                "resource_name": "Sikemux",
            }),
        ),
        (
            Method::GET,
            "/.well-known/oauth-authorization-server"
            | "/.well-known/oauth-authorization-server/mcp"
            | "/.well-known/openid-configuration",
        ) => json_reply(
            StatusCode::OK,
            &json!({
                "issuer": base,
                "authorization_endpoint": format!("{base}/authorize"),
                "token_endpoint": format!("{base}/token"),
                "registration_endpoint": format!("{base}/register"),
                "response_types_supported": ["code"],
                "grant_types_supported": ["authorization_code"],
                "code_challenge_methods_supported": ["S256"],
                "token_endpoint_auth_methods_supported": ["none"],
                "scopes_supported": ["chats:read", "chats:write"],
            }),
        ),
        (Method::POST, "/register") if not_from_a_page(&request) => {
            let body = read_body(request.into_body()).await;
            let Some(body) = body.and_then(|body| serde_json::from_slice::<Value>(&body).ok())
            else {
                return json_reply(
                    StatusCode::BAD_REQUEST,
                    &json!({ "error": "invalid_client_metadata", "error_description": "the body must be JSON" }),
                );
            };
            match core.outside().register(&body) {
                Ok(registered) => json_reply(StatusCode::CREATED, &registered),
                Err(refusal) => oauth_refusal(StatusCode::BAD_REQUEST, &refusal),
            }
        }
        (Method::GET, "/authorize") => {
            match authorize_query(&query).and_then(|asked| core.outside().authorize(asked)) {
                Ok(waiting) => {
                    changed(&core);
                    see_other(&format!("/authorize/wait?id={waiting}"))
                }
                Err(refusal) => reply(
                    StatusCode::BAD_REQUEST,
                    "text/html; charset=utf-8",
                    page(
                        "Sikemux could not start this sign-in",
                        &refusal.description,
                        None,
                    ),
                ),
            }
        }
        (Method::GET, "/authorize/wait") => {
            let id = query
                .iter()
                .find(|(key, _)| key == "id")
                .map(|(_, id)| id.clone())
                .unwrap_or_default();
            match core.outside().back_to_client(&id) {
                Ok(Some(back)) => {
                    changed(&core);
                    see_other(&back)
                }
                Ok(None) => match core.outside().waiting_shown(&id) {
                    Some(shown) => reply(StatusCode::OK, "text/html; charset=utf-8", page(
                        &format!("Allow {} in Sikemux", shown.name),
                        &format!("Sikemux asks whether {} may use your chats. Check that it shows {} there, then choose what it may do. This page goes on by itself.", shown.name, shown.check),
                        Some(&shown.check),
                    )),
                    None => reply(StatusCode::GONE, "text/html; charset=utf-8", page("This sign-in is over", "Start it again from the app.", None)),
                },
                Err(error) => reply(StatusCode::INTERNAL_SERVER_ERROR, "text/html; charset=utf-8", page("Sikemux could not finish this sign-in", &error.to_string(), None)),
            }
        }
        (Method::POST, "/token") if not_from_a_page(&request) => {
            let body = read_body(request.into_body()).await.unwrap_or_default();
            match core
                .outside()
                .token(&pairs(&String::from_utf8_lossy(&body)))
            {
                Ok(token) => {
                    changed(&core);
                    json_reply(StatusCode::OK, &token)
                }
                Err(refusal) => oauth_refusal(StatusCode::BAD_REQUEST, &refusal),
            }
        }
        (Method::POST, "/mcp") if not_from_a_page(&request) => mcp(&core, &base, request).await,
        (_, "/mcp") => {
            let mut response = reply(StatusCode::METHOD_NOT_ALLOWED, "text/plain", "");
            response
                .headers_mut()
                .insert("allow", HeaderValue::from_static("POST"));
            response
        }
        (_, "/register" | "/token") => reply(
            StatusCode::FORBIDDEN,
            "text/plain",
            "Web pages cannot sign in to Sikemux",
        ),
        _ => reply(StatusCode::NOT_FOUND, "text/plain", "Not found"),
    }
}

fn unauthorized(base: &str, presented: bool) -> Response<Body> {
    let mut response = json_reply(
        StatusCode::UNAUTHORIZED,
        &json!({ "error": "invalid_token" }),
    );
    let mut challenge =
        format!("Bearer resource_metadata=\"{base}/.well-known/oauth-protected-resource\"");
    if presented {
        challenge.push_str(", error=\"invalid_token\"");
    }
    if let Ok(value) = HeaderValue::from_str(&challenge) {
        response.headers_mut().insert("www-authenticate", value);
    }
    response
}

async fn mcp(core: &Arc<Core>, base: &str, request: Request<Incoming>) -> Response<Body> {
    let token = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value
                .strip_prefix("Bearer ")
                .map(str::trim)
                .map(str::to_owned)
        });
    let Some(caller) = token
        .as_deref()
        .and_then(|token| core.outside().caller(token))
    else {
        return unauthorized(base, token.is_some());
    };
    let Some(body) = read_body(request.into_body()).await else {
        return reply(StatusCode::PAYLOAD_TOO_LARGE, "text/plain", "");
    };
    let Ok(message) = serde_json::from_slice::<Value>(&body) else {
        return json_reply(
            StatusCode::BAD_REQUEST,
            &rpc_error(Value::Null, -32700, "the body is not JSON"),
        );
    };
    let Some(id) = message.get("id").cloned() else {
        return reply(StatusCode::ACCEPTED, "text/plain", "");
    };
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => Ok(initialize(&params, &core.config.build.version)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({
            "tools": chats::tools(caller.access).map(|tool| json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": (tool.schema)(),
            })).collect::<Vec<_>>(),
        })),
        "tools/call" => Ok(call_tool(core, &caller, &params).await),
        _ => Err(rpc_error(
            id.clone(),
            -32601,
            &format!("Sikemux has no method {method}"),
        )),
    };
    match result {
        Ok(result) => json_reply(
            StatusCode::OK,
            &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        ),
        Err(error) => json_reply(StatusCode::OK, &error),
    }
}

fn initialize(params: &Value, version_of_sikemux: &str) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked
        .filter(|asked| PROTOCOLS.contains(asked))
        .unwrap_or(PROTOCOLS[1]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "sikemux", "title": "Sikemux", "version": version_of_sikemux },
        "instructions": INSTRUCTIONS,
    })
}

async fn call_tool(core: &Arc<Core>, caller: &Caller, params: &Value) -> Value {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    match chats::call(core, caller, name, &args).await {
        Ok(result) => json!({
            "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_default() }],
            "structuredContent": result,
            "isError": false,
        }),
        Err(error) => json!({
            "content": [{ "type": "text", "text": error }],
            "isError": true,
        }),
    }
}

fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The page the browser shows while the person answers in the app. It reloads itself until
/// they do, and gives up with the wait.
fn page(title: &str, body: &str, check: Option<&str>) -> String {
    let refresh = match check {
        Some(_) => "<meta http-equiv=\"refresh\" content=\"2\">",
        None => "",
    };
    let check = check.map_or_else(String::new, |check| {
        format!("<p class=\"check\">{}</p>", escape(check))
    });
    let minutes = ANSWER_WITHIN.as_secs() / 60;
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\">{refresh}<title>{title}</title><style>:root{{color-scheme:dark;}}body{{margin:0;min-height:100vh;display:grid;place-items:center;background:#0d0d10;color:#ececf1;font:14px/1.5 -apple-system,BlinkMacSystemFont,\"Figtree\",sans-serif;}}main{{max-width:380px;padding:24px;}}h1{{font-size:16px;font-weight:600;margin:0 0 8px;}}p{{margin:0 0 12px;color:#a6a6b0;}}.check{{font:600 24px/1 ui-monospace,monospace;letter-spacing:.2em;color:#a277ff;margin:16px 0;}}small{{color:#6e6e78;}}</style></head><body><main><h1>{title}</h1><p>{body}</p>{check}<small>A sign-in waits {minutes} minutes for an answer.</small></main></body></html>",
        title = escape(title),
        body = escape(body),
    )
}
