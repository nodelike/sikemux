//! The loopback proxy agents reach a connected model provider through. An agent holds a
//! token that works only while its process runs; the core puts the provider's key on each
//! request here, so the key never enters an agent's environment.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use hyper::body::Incoming;
use hyper::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sikemux_process::sync::Locked;
use sikemux_process::user_environment::shell_var;
use tokio::net::{TcpListener, TcpStream};

use sikemux_wire::protocol::ChatLaunch;

use super::Core;
use crate::launch::providers::{
    AnthropicAuth, Provider, Providers, MODEL_PROXY_SLOT, MODEL_TOKEN_SLOT,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const ACCEPT_RETRY: Duration = Duration::from_millis(50);
const X_API_KEY: HeaderName = HeaderName::from_static("x-api-key");

/// Headers about one connection, which stop at the proxy, with `host` and what the agent sent
/// as its key.
const NOT_FORWARDED: [&str; 10] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
    "x-api-key",
];

type Grants = Mutex<HashMap<Vec<u8>, String>>;

#[derive(Default)]
pub(crate) struct ModelProxy {
    /// Bound when the first agent needs it, so a person with no provider pays nothing.
    serving: Mutex<Option<Serving>>,
    grants: Arc<Grants>,
}

#[derive(Clone, Copy)]
struct Serving {
    port: u16,
    fd: RawFd,
}

/// An agent's token, good for one provider until this is dropped.
pub(crate) struct Grant {
    token: String,
    provider: String,
    grants: Arc<Grants>,
}

/// A [`Grant`] as the core that takes a running agent over is told it.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KeptGrant {
    token: String,
    provider: String,
}

impl Grant {
    /// Lets go of the token without revoking it, for the core that takes the agent up.
    pub(crate) fn hand_over(mut self) -> KeptGrant {
        KeptGrant {
            token: std::mem::take(&mut self.token),
            provider: std::mem::take(&mut self.provider),
        }
    }
}

impl Drop for Grant {
    fn drop(&mut self) {
        if !self.token.is_empty() {
            self.grants.locked().remove(&digest(&self.token));
        }
    }
}

fn digest(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

impl ModelProxy {
    /// `launch` with the proxy's address and a new token in place of its slots, and the grant
    /// that keeps the token good. `None` for a launch that reaches its provider directly.
    pub(crate) fn grant(
        &self,
        core: &Arc<Core>,
        launch: &ChatLaunch,
    ) -> Result<Option<(Grant, ChatLaunch)>, String> {
        let carries_token = launch.env.values().any(|value| value == MODEL_TOKEN_SLOT);
        let (true, Some(provider)) = (carries_token, launch.model_provider.clone()) else {
            return Ok(None);
        };
        let port = self
            .serving(core)
            .map_err(|error| format!("Sikemux could not reach the model provider: {error}"))?
            .port;
        let token = super::tools::new_token();
        self.grants
            .locked()
            .insert(digest(&token), provider.clone());
        let origin = format!("http://{}:{port}", Ipv4Addr::LOCALHOST);
        let filled = |value: &str| {
            value
                .replace(MODEL_PROXY_SLOT, &origin)
                .replace(MODEL_TOKEN_SLOT, &token)
        };
        let mut resolved = launch.clone();
        for value in resolved.env.values_mut() {
            *value = filled(value);
        }
        for arg in &mut resolved.args {
            *arg = filled(arg);
        }
        if outbound_proxy().is_some() {
            let bypass = match shell_var("NO_PROXY").or_else(|| shell_var("no_proxy")) {
                Some(listed) => format!("{listed},{}", Ipv4Addr::LOCALHOST),
                None => Ipv4Addr::LOCALHOST.to_string(),
            };
            resolved.env.insert("NO_PROXY".into(), bypass.clone());
            resolved.env.insert("no_proxy".into(), bypass);
        }
        let grant = Grant {
            token,
            provider,
            grants: self.grants.clone(),
        };
        Ok(Some((grant, resolved)))
    }

    /// Makes the token of an agent an earlier core ran good here.
    pub(crate) fn restore(&self, kept: KeptGrant) -> Grant {
        self.grants
            .locked()
            .insert(digest(&kept.token), kept.provider.clone());
        Grant {
            token: kept.token,
            provider: kept.provider,
            grants: self.grants.clone(),
        }
    }

    fn serving(&self, core: &Arc<Core>) -> std::io::Result<Serving> {
        let mut serving = self.serving.locked();
        if let Some(serving) = *serving {
            return Ok(serving);
        }
        let listener = std::net::TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?;
        let bound = self.serve(core, listener)?;
        *serving = Some(bound);
        Ok(bound)
    }

    fn serve(&self, core: &Arc<Core>, listener: std::net::TcpListener) -> std::io::Result<Serving> {
        listener.set_nonblocking(true)?;
        let listener = TcpListener::from_std(listener)?;
        let serving = Serving {
            port: listener.local_addr()?.port(),
            fd: listener.as_raw_fd(),
        };
        tokio::spawn(accept(Arc::downgrade(core), self.grants.clone(), listener));
        Ok(serving)
    }

    /// The listening socket, which a core that replaces this one keeps serving, so a running
    /// agent's address stays good. `None` before any agent has used the proxy.
    pub(crate) fn listener_fd(&self) -> Option<RawFd> {
        self.serving.locked().map(|serving| serving.fd)
    }

    /// Keeps serving the socket an earlier core handed over.
    ///
    /// # Safety
    /// `fd` is an open listening socket this process owns and nothing else will close.
    pub(crate) unsafe fn adopt(&self, core: &Arc<Core>, fd: RawFd) -> std::io::Result<()> {
        // SAFETY: the caller hands this descriptor over, open and owned by nothing else.
        let listener = unsafe { std::net::TcpListener::from_raw_fd(fd) };
        let bound = self.serve(core, listener)?;
        *self.serving.locked() = Some(bound);
        Ok(())
    }
}

async fn accept(core: Weak<Core>, grants: Arc<Grants>, listener: TcpListener) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            tokio::time::sleep(ACCEPT_RETRY).await;
            continue;
        };
        tokio::spawn(connection(stream, core.clone(), grants.clone()));
    }
}

async fn connection(stream: TcpStream, core: Weak<Core>, grants: Arc<Grants>) {
    // A reply's small pieces go out as they come, not held back to fill a packet.
    let _ = stream.set_nodelay(true);
    let service = service_fn(move |request| {
        let core = core.clone();
        let grants = grants.clone();
        async move { Ok::<_, Infallible>(answer(&core, &grants, request).await) }
    });
    let _ = http1::Builder::new()
        .serve_connection(TokioIo::new(stream), service)
        .await;
}

async fn answer(
    core: &Weak<Core>,
    grants: &Grants,
    request: Request<Incoming>,
) -> Response<reqwest::Body> {
    match forward(core, grants, request).await {
        Ok(response) => response,
        Err((status, message)) => refusal(status, &message),
    }
}

/// In OpenAI's error shape, which every agent here can show.
fn refusal(status: StatusCode, message: &str) -> Response<reqwest::Body> {
    let body = serde_json::json!({ "error": { "message": message, "type": "sikemux_proxy" } });
    let mut response = Response::new(reqwest::Body::from(body.to_string()));
    *response.status_mut() = status;
    response
}

type Refused = (StatusCode, String);

fn refused(status: StatusCode, message: impl Into<String>) -> Refused {
    (status, message.into())
}

async fn forward(
    core: &Weak<Core>,
    grants: &Grants,
    request: Request<Incoming>,
) -> Result<Response<reqwest::Body>, Refused> {
    let provider_id = presented_token(request.headers())
        .and_then(|token| grants.locked().get(&digest(token)).cloned())
        .ok_or_else(|| {
            refused(
                StatusCode::UNAUTHORIZED,
                "This token's chat has ended, or Sikemux never issued it",
            )
        })?;
    let route = Route::of(request.uri().path())
        .filter(|route| route.provider == provider_id)
        .ok_or_else(|| refused(StatusCode::FORBIDDEN, "This token is for another provider"))?;
    let core = core
        .upgrade()
        .ok_or_else(|| refused(StatusCode::SERVICE_UNAVAILABLE, "Sikemux is stopping"))?;
    let (provider, key) = provider_and_key(core, provider_id).await?;
    let target = route
        .target(&provider, request.uri().query())
        .ok_or_else(|| refused(StatusCode::NOT_FOUND, "That provider has no such API"))?;
    let api = route.api;
    let (parts, body) = request.into_parts();
    let mut headers = parts.headers;
    for name in NOT_FORWARDED {
        headers.remove(name);
    }
    headers.remove(AUTHORIZATION);
    let (name, value) = provider_credential(api, &provider, &key)?;
    headers.insert(name, value);
    let upstream = client()
        .request(parts.method, target)
        .headers(headers)
        .body(reqwest::Body::wrap(body))
        .send()
        .await
        .map_err(|_| {
            refused(
                StatusCode::BAD_GATEWAY,
                format!("{} could not be reached", provider.label),
            )
        })?;
    let mut response = Response::<reqwest::Body>::from(upstream);
    for name in NOT_FORWARDED {
        response.headers_mut().remove(name);
    }
    Ok(response)
}

/// Blocks, as reading a key can start the Keychain, so it runs off the async threads.
async fn provider_and_key(core: Arc<Core>, id: String) -> Result<(Arc<Provider>, String), Refused> {
    let found = tokio::task::spawn_blocking(move || {
        let providers: &Providers = core.workspaces.providers()?;
        Some((providers.find(&id)?, providers.key(&id)))
    })
    .await;
    match found {
        Ok(Some((provider, Some(key)))) => Ok((provider, key)),
        Ok(Some((provider, None))) => Err(refused(
            StatusCode::UNAUTHORIZED,
            format!("Connect {} in Settings to run chats on it", provider.label),
        )),
        _ => Err(refused(
            StatusCode::NOT_FOUND,
            "Sikemux no longer knows this provider",
        )),
    }
}

fn presented_token(headers: &HeaderMap) -> Option<&str> {
    let bearer = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    bearer.or_else(|| headers.get(X_API_KEY)?.to_str().ok())
}

fn provider_credential(
    api: Api,
    provider: &Provider,
    key: &str,
) -> Result<(HeaderName, HeaderValue), Refused> {
    let (name, value) = match (api, provider.anthropic.as_ref().map(|api| api.auth)) {
        (Api::Anthropic, Some(AnthropicAuth::XApiKey)) => (X_API_KEY, key.to_owned()),
        _ => (AUTHORIZATION, format!("Bearer {key}")),
    };
    let mut value = HeaderValue::from_str(&value).map_err(|_| {
        refused(
            StatusCode::BAD_GATEWAY,
            format!("The key saved for {} cannot be sent", provider.label),
        )
    })?;
    value.set_sensitive(true);
    Ok((name, value))
}

#[derive(Clone, Copy)]
enum Api {
    OpenAi,
    Anthropic,
}

/// What a request's path names: `/<provider>/<api>/<the rest of the provider's path>`.
struct Route<'a> {
    provider: &'a str,
    api: Api,
    rest: &'a str,
}

impl<'a> Route<'a> {
    fn of(path: &'a str) -> Option<Self> {
        let mut parts = path.strip_prefix('/')?.splitn(3, '/');
        let provider = parts.next()?;
        let api = match parts.next()? {
            "openai" => Api::OpenAi,
            "anthropic" => Api::Anthropic,
            _ => return None,
        };
        Some(Self {
            provider,
            api,
            rest: parts.next().unwrap_or_default(),
        })
    }

    /// The provider's address for this request. Must stay on the provider's own host, since
    /// the key goes wherever this points.
    fn target(&self, provider: &Provider, query: Option<&str>) -> Option<url::Url> {
        let base = match self.api {
            Api::OpenAi => provider.openai.as_str(),
            Api::Anthropic => provider.anthropic.as_ref()?.url.as_str(),
        };
        let mut text = format!("{}/{}", base.trim_end_matches('/'), self.rest);
        if let Some(query) = query {
            text.push('?');
            text.push_str(query);
        }
        let target = url::Url::parse(&text).ok()?;
        (target.origin() == url::Url::parse(base).ok()?.origin()).then_some(target)
    }
}

/// The proxy an agent would have gone through itself, from the person's shell, as a core
/// opened from the Dock has none of its variables.
fn outbound_proxy() -> Option<String> {
    ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
        .into_iter()
        .find_map(shell_var)
}

/// Passes the provider's bytes as they are, follows no redirect so the key cannot be carried
/// to another host, and has no overall limit since a streamed reply can run for many minutes.
fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let mut builder = reqwest::Client::builder()
            .no_gzip()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT);
        if let Some(proxy) = outbound_proxy().and_then(|url| reqwest::Proxy::all(url).ok()) {
            let exempt = shell_var("NO_PROXY").or_else(|| shell_var("no_proxy"));
            builder = builder
                .proxy(proxy.no_proxy(exempt.as_deref().and_then(reqwest::NoProxy::from_string)));
        }
        builder.build().unwrap_or_default()
    })
}
