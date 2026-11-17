//! Signing in an MCP client with OAuth 2.1: it registers itself, the person allows it in the
//! app while the browser waits, and it trades the code it is sent back for a token. Only
//! clients that return to this computer may sign in, so a token never leaves it.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sikemux_process::clock::epoch_ms;
use sikemux_process::sync::Locked;
use url::Url;

use super::{clean_name, digest, random_text, Answer, Inner, Outside, Via, MAX_REGISTRATIONS};
use crate::server::CoreResult;

const CODE_LASTS: Duration = Duration::from_secs(300);
const MAX_REDIRECTS: usize = 8;
const MAX_REDIRECT_BYTES: usize = 2048;

/// A client that registered itself.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Registration {
    client_id: String,
    name: String,
    redirect_uris: Vec<String>,
    at: u64,
}

/// A code the browser carries back to the client, good once.
pub(super) struct Code {
    client_id: String,
    redirect_uri: String,
    challenge: String,
    /// The answer of the sign-in it came from, whose program it lets in.
    waiting: String,
    until: u64,
}

/// What the client asked when it sent the person to sign in.
#[derive(Clone)]
pub(crate) struct Authorize {
    pub client_id: String,
    pub redirect_uri: String,
    pub challenge: String,
    pub state: Option<String>,
}

/// An OAuth error, which goes back to the client in its standard shape.
pub(crate) struct Refusal {
    pub code: &'static str,
    pub description: String,
}

fn refusal(code: &'static str, description: impl Into<String>) -> Refusal {
    Refusal {
        code,
        description: description.into(),
    }
}

impl Refusal {
    pub(crate) fn body(&self) -> Value {
        json!({ "error": self.code, "error_description": self.description })
    }
}

/// Whether `uri` returns to this computer: plain http to a loopback address, as RFC 8252 has
/// native apps do.
fn on_this_computer(uri: &str) -> bool {
    let Ok(url) = Url::parse(uri) else {
        return false;
    };
    let loopback = match url.host() {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    url.scheme() == "http" && loopback && url.username().is_empty() && url.fragment().is_none()
}

/// A redirect that matches a registered one, the port aside, as RFC 8252 lets a native app
/// pick its port each time.
fn registered(registration: &Registration, uri: &str) -> bool {
    let without_port = |uri: &str| {
        let mut url = Url::parse(uri).ok()?;
        url.set_port(None).ok()?;
        Some(url)
    };
    let asked = without_port(uri);
    asked.is_some()
        && registration
            .redirect_uris
            .iter()
            .any(|kept| without_port(kept) == asked)
}

impl Outside {
    /// Registers a client by RFC 7591, from the JSON it posted.
    pub(crate) fn register(&self, body: &Value) -> Result<Value, Refusal> {
        let uris: Vec<String> = body
            .get("redirect_uris")
            .and_then(Value::as_array)
            .map(|uris| {
                uris.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        if uris.is_empty() || uris.len() > MAX_REDIRECTS {
            return Err(refusal(
                "invalid_redirect_uri",
                "redirect_uris must list one to eight addresses",
            ));
        }
        if let Some(far) = uris
            .iter()
            .find(|uri| uri.len() > MAX_REDIRECT_BYTES || !on_this_computer(uri))
        {
            return Err(refusal(
                "invalid_redirect_uri",
                format!("Sikemux lets in only apps on this computer, and {far} is not a localhost address"),
            ));
        }
        let name = clean_name(
            body.get("client_name")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        let registration = Registration {
            client_id: random_text(16)
                .map_err(|error| refusal("server_error", error.to_string()))?,
            name,
            redirect_uris: uris,
            at: epoch_ms(),
        };
        let answer = json!({
            "client_id": registration.client_id,
            "client_name": registration.name,
            "redirect_uris": registration.redirect_uris,
            "grant_types": ["authorization_code"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
            "client_id_issued_at": registration.at / 1000,
        });
        let mut inner = self.inner.locked();
        let kept = &mut inner.stored.registrations;
        if kept.len() >= MAX_REGISTRATIONS {
            kept.remove(0);
        }
        kept.push(registration);
        self.save(&inner.stored);
        Ok(answer)
    }

    /// Checks what the client asked to sign in with, and puts it before the person. Answers
    /// with the id the browser waits on.
    pub(crate) fn authorize(&self, asked: Authorize) -> Result<String, Refusal> {
        let registration = {
            let inner = self.inner.locked();
            inner
                .stored
                .registrations
                .iter()
                .find(|kept| kept.client_id == asked.client_id)
                .cloned()
        }
        .ok_or_else(|| {
            refusal(
                "invalid_client",
                "Sikemux does not know this app; remove it from the app and add it again",
            )
        })?;
        if !on_this_computer(&asked.redirect_uri) || !registered(&registration, &asked.redirect_uri)
        {
            return Err(refusal(
                "invalid_request",
                "redirect_uri is not one this app registered",
            ));
        }
        let shown = Url::parse(&asked.redirect_uri).ok().map(|url| {
            format!(
                "{}:{}",
                url.host_str().unwrap_or_default(),
                url.port_or_known_default().unwrap_or(80)
            )
        });
        let waiting = self
            .ask(&registration.name, Via::Mcp, shown)
            .map_err(|error| refusal("temporarily_unavailable", error.to_string()))?;
        let mut inner = self.inner.locked();
        let Inner {
            waiting: open,
            authorizing,
            ..
        } = &mut *inner;
        authorizing.retain(|id, _| open.iter().any(|waiting| &waiting.shown.id == id));
        authorizing.insert(waiting.clone(), asked);
        Ok(waiting)
    }

    /// Where the browser goes once the person answered `waiting`: back to the client with a
    /// code, or with why not. None while they have not answered.
    pub(crate) fn back_to_client(&self, waiting: &str) -> CoreResult<Option<String>> {
        let answer = self.answer_of(waiting);
        if answer == Answer::Waiting {
            return Ok(None);
        }
        let asked = self.inner.locked().authorizing.remove(waiting);
        let Some(asked) = asked else {
            return Ok(None);
        };
        let mut back = Url::parse(&asked.redirect_uri)
            .map_err(|_| crate::server::CoreError::from("the app's address is not a URL"))?;
        {
            let mut query = back.query_pairs_mut();
            match answer {
                Answer::Allowed(_) => {
                    let code = random_text(32)?;
                    let mut inner = self.inner.locked();
                    let now = epoch_ms();
                    inner.codes.retain(|_, code| code.until > now);
                    inner.codes.insert(
                        digest(&code),
                        Code {
                            client_id: asked.client_id.clone(),
                            redirect_uri: asked.redirect_uri.clone(),
                            challenge: asked.challenge.clone(),
                            waiting: waiting.to_owned(),
                            until: now + CODE_LASTS.as_millis() as u64,
                        },
                    );
                    query.append_pair("code", &code);
                }
                _ => {
                    query.append_pair("error", "access_denied");
                }
            }
            if let Some(state) = &asked.state {
                query.append_pair("state", state);
            }
        }
        Ok(Some(back.into()))
    }

    /// Trades a code for a token by RFC 6749 with PKCE, from the form the client posted.
    pub(crate) fn token(&self, form: &[(String, String)]) -> Result<Value, Refusal> {
        let field = |name: &str| {
            form.iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        if field("grant_type") != Some("authorization_code") {
            return Err(refusal(
                "unsupported_grant_type",
                "Sikemux gives tokens only for an authorization code",
            ));
        }
        let given = field("code").ok_or_else(|| refusal("invalid_request", "code is missing"))?;
        let verifier = field("code_verifier")
            .ok_or_else(|| refusal("invalid_request", "code_verifier is missing"))?;
        let code = self
            .inner
            .locked()
            .codes
            .remove(&digest(given))
            .filter(|code| code.until > epoch_ms())
            .ok_or_else(|| refusal("invalid_grant", "this code is used, expired or unknown"))?;
        let matches = field("client_id").is_none_or(|id| id == code.client_id)
            && field("redirect_uri").is_none_or(|uri| uri == code.redirect_uri)
            && challenge_of(verifier) == code.challenge;
        if !matches {
            return Err(refusal(
                "invalid_grant",
                "this code was given for another sign-in",
            ));
        }
        let (token, client) = self
            .admit(&code.waiting)
            .map_err(|error| refusal("invalid_grant", error.to_string()))?;
        Ok(json!({
            "access_token": token,
            "token_type": "Bearer",
            "expires_in": client.expires_at.saturating_sub(epoch_ms()) / 1000,
            "scope": scope_of(client.access),
        }))
    }
}

pub(crate) fn scope_of(access: sikemux_wire::protocol::DeviceAccess) -> &'static str {
    match access {
        sikemux_wire::protocol::DeviceAccess::Full => "chats:read chats:write",
        sikemux_wire::protocol::DeviceAccess::Watch => "chats:read",
    }
}

/// The S256 challenge of `verifier`.
fn challenge_of(verifier: &str) -> String {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// What the client asked, from the query of `/authorize`.
pub(crate) fn authorize_query(query: &[(String, String)]) -> Result<Authorize, Refusal> {
    let field = |name: &str| {
        query
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    if field("response_type").as_deref() != Some("code") {
        return Err(refusal(
            "unsupported_response_type",
            "response_type must be code",
        ));
    }
    if field("code_challenge_method").as_deref() != Some("S256") {
        return Err(refusal("invalid_request", "Sikemux needs PKCE with S256"));
    }
    let challenge = field("code_challenge")
        .filter(|challenge| (43..=128).contains(&challenge.len()))
        .ok_or_else(|| refusal("invalid_request", "code_challenge is missing"))?;
    Ok(Authorize {
        client_id: field("client_id")
            .ok_or_else(|| refusal("invalid_request", "client_id is missing"))?,
        redirect_uri: field("redirect_uri")
            .ok_or_else(|| refusal("invalid_request", "redirect_uri is missing"))?,
        challenge,
        state: field("state"),
    })
}
