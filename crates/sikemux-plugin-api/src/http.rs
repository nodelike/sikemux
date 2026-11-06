//! The HTTP client every plugin talks to its service with: kept warm, with
//! the same deadlines everywhere, a cap on requests at once, and a cap on how
//! much one answer may be.

use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use reqwest::header::HeaderMap;
use reqwest::{Client, ClientBuilder, Method, RequestBuilder, Response, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::ratelimit::{LimitHeaders, RateLimits};
use crate::service::{Service, ServiceError};

const IDLE: Duration = Duration::from_secs(25);
/// Split across a host's addresses, so one that never answers costs a few
/// seconds before the next is tried rather than the whole request.
const CONNECT: Duration = Duration::from_secs(4);
const WHOLE_ANSWER: Duration = Duration::from_secs(30);
/// Files can be large and slow, so a transfer has no overall deadline, only
/// one on going quiet.
const QUIET_TRANSFER: Duration = Duration::from_secs(60);
const MAX_REDIRECTS: usize = 5;
/// The most of an answer anything reads whole.
pub const MAX_ANSWER: usize = 16 * 1024 * 1024;
/// Enough for who a token belongs to.
pub const MAX_IDENTITY: usize = 1024 * 1024;

#[derive(Clone, Copy)]
pub enum Redirects {
    /// A redirect is the answer, such as a sign-in page in front of the service.
    Refuse,
    /// A redirect keeps the token, so only one over https on the same server is followed.
    SameHost,
    /// Followed anywhere; the token is dropped on the way to another server.
    Follow,
}

#[derive(Clone, Copy)]
pub enum Deadline {
    WholeAnswer,
    QuietTransfer,
}

/// The settings every plugin's client starts from.
pub fn builder<S: Service>(redirects: Redirects, deadline: Deadline) -> ClientBuilder {
    let policy = match redirects {
        Redirects::Refuse => reqwest::redirect::Policy::none(),
        Redirects::Follow => reqwest::redirect::Policy::limited(MAX_REDIRECTS),
        Redirects::SameHost => reqwest::redirect::Policy::custom(|attempt| {
            let follow = attempt.previous().len() <= MAX_REDIRECTS
                && attempt
                    .previous()
                    .first()
                    .is_some_and(|first| same_host(attempt.url(), first));
            if follow {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }),
    };
    let builder = Client::builder()
        .pool_idle_timeout(IDLE)
        .connect_timeout(CONNECT)
        .redirect(policy)
        .user_agent(format!("sikemux-{}/0.1", S::ID));
    match deadline {
        Deadline::WholeAnswer => builder.timeout(WHOLE_ANSWER),
        Deadline::QuietTransfer => builder.read_timeout(QUIET_TRANSFER),
    }
}

pub fn same_host(next: &Url, first: &Url) -> bool {
    next.scheme() == "https"
        && next.host_str() == first.host_str()
        && next.port_or_known_default() == first.port_or_known_default()
}

fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// A bare host name out of whatever was pasted for one, with a port only when
/// one was given: a scheme, a user and a path are trimmed off. For a service
/// that is always reached over https, so none of those decide where a token goes.
pub fn bare_host(typed: &str) -> Option<String> {
    let typed = typed.trim();
    let after_scheme = typed.split_once("://").map_or(typed, |(_, rest)| rest);
    let authority = after_scheme.split(['/', '?', '#']).next()?;
    let authority = authority.split('@').next_back()?;
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    let port_ok = port.is_none_or(|port| port.parse::<u16>().is_ok());
    (port_ok && host.len() <= 253 && host.split('.').all(valid_label))
        .then(|| authority.to_ascii_lowercase())
}

/// When a server a token is sent to may be reached over plain http.
#[derive(Clone, Copy)]
pub enum PlainHttp {
    Refused,
    /// `localhost` and its addresses, where nothing crosses a network.
    OnThisMachine,
    /// The person said so, and the plugin answers for where it may lead.
    Acknowledged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressProblem {
    NotAnAddress,
    HoldsCredentials,
    Scheme,
}

/// The whole address of a server a token is sent to, as a person typed it.
pub fn base_url(typed: &str, plain: PlainHttp) -> Result<Url, AddressProblem> {
    let url = Url::parse(typed.trim()).map_err(|_| AddressProblem::NotAnAddress)?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AddressProblem::HoldsCredentials);
    }
    let host = url.host_str().ok_or(AddressProblem::NotAnAddress)?;
    let local = matches!(host, "localhost" | "127.0.0.1" | "[::1]");
    match (url.scheme(), plain) {
        ("https", _) | ("http", PlainHttp::Acknowledged) => Ok(url),
        ("http", PlainHttp::OnThisMachine) if local => Ok(url),
        _ => Err(AddressProblem::Scheme),
    }
}

/// One warm client for a service, the bound on its requests in flight, and
/// its rate limit, when its answers report one.
pub struct Http<S> {
    answers: OnceLock<Option<Client>>,
    transfers: OnceLock<Option<Client>>,
    redirects: Redirects,
    permits: Semaphore,
    limits: Option<RateLimits>,
    service: PhantomData<fn() -> S>,
}

impl<S: Service> Http<S> {
    pub const fn new(redirects: Redirects, in_flight: usize) -> Self {
        Self {
            answers: OnceLock::new(),
            transfers: OnceLock::new(),
            redirects,
            permits: Semaphore::const_new(in_flight),
            limits: None,
            service: PhantomData,
        }
    }

    pub const fn with_limits(redirects: Redirects, in_flight: usize, limit: LimitHeaders) -> Self {
        Self {
            answers: OnceLock::new(),
            transfers: OnceLock::new(),
            redirects,
            permits: Semaphore::const_new(in_flight),
            limits: Some(RateLimits::new(limit)),
            service: PhantomData,
        }
    }

    pub fn client(&self) -> Result<&Client, ServiceError<S>> {
        self.started(&self.answers, Deadline::WholeAnswer)
    }

    /// For logs and files, which share the request slots but may take as long as they keep arriving.
    pub fn transfer_client(&self) -> Result<&Client, ServiceError<S>> {
        self.started(&self.transfers, Deadline::QuietTransfer)
    }

    fn started<'a>(
        &self,
        client: &'a OnceLock<Option<Client>>,
        deadline: Deadline,
    ) -> Result<&'a Client, ServiceError<S>> {
        client
            .get_or_init(|| builder::<S>(self.redirects, deadline).build().ok())
            .as_ref()
            .ok_or_else(|| ServiceError::Transport("could not start the HTTP client".into()))
    }

    /// Runs `work` once one of the request slots is free.
    pub async fn limited<T>(&self, work: impl Future<Output = T>) -> T {
        let _permit = self.permits.acquire().await;
        work.await
    }

    pub async fn send(&self, request: RequestBuilder) -> Result<Response, ServiceError<S>> {
        Ok(self.limited(request.send()).await?)
    }

    /// Sends a request and reads its whole answer, with no rate limit in the way.
    pub async fn ask(
        &self,
        request: RequestBuilder,
        read: Read,
    ) -> Result<Answer, ServiceError<S>> {
        let response = self.send(request).await?;
        let status = response.status();
        let headers = response.headers().clone();
        let (bytes, cut) = read.of(response).await?;
        Ok(Answer {
            status,
            headers,
            bytes,
            cut,
            spent: None,
        })
    }

    /// Refuses a request while `account`'s limit is spent, and otherwise sends
    /// it. A limit that resets within a few seconds is waited out and the
    /// request sent once more.
    pub async fn exchange(
        &self,
        account: &str,
        request: RequestBuilder,
        read: Read,
    ) -> Result<Answer, ServiceError<S>> {
        let mut request = request;
        let mut waited = false;
        loop {
            self.check(account)?;
            let again = request.try_clone().filter(|_| !waited);
            let mut answer = self.ask(request, read).await?;
            let pause = self.limits.as_ref().and_then(|limits| {
                limits.short_wait(account, answer.status, &answer.headers, &answer.bytes)
            });
            match (pause, again) {
                (Some(pause), Some(retry)) => {
                    waited = true;
                    tokio::time::sleep(pause).await;
                    request = retry;
                }
                _ => {
                    answer.spent =
                        self.observe(account, answer.status, &answer.headers, &answer.bytes);
                    return Ok(answer);
                }
            }
        }
    }

    /// Refuses a request while `account`'s limit is spent.
    pub fn check(&self, account: &str) -> Result<(), ServiceError<S>> {
        self.limits
            .as_ref()
            .map_or(Ok(()), |limits| limits.check(account))
    }

    /// Keeps what an answer says of the limit, and says how long to hold off if it is spent.
    pub fn observe(
        &self,
        account: &str,
        status: StatusCode,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Option<u64> {
        self.limits
            .as_ref()
            .and_then(|limits| limits.observe(account, status, headers, body))
    }

    /// What a refused answer that was not read yet means, such as a file's.
    pub async fn refusal(&self, account: &str, response: Response) -> ServiceError<S> {
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = read_body(response, MAX_ANSWER).await.unwrap_or_default();
        let spent = self.observe(account, status, &headers, &bytes);
        S::refused(status, &bytes, spent)
    }
}

/// How much of an answer to keep.
#[derive(Clone, Copy)]
pub enum Read {
    Whole(usize),
    /// Only the end, such as of a long log.
    Tail(usize),
}

impl Read {
    pub const ALL: Self = Self::Whole(MAX_ANSWER);
    pub const IDENTITY: Self = Self::Whole(MAX_IDENTITY);

    pub async fn of<S: Service>(
        self,
        response: Response,
    ) -> Result<(Vec<u8>, bool), ServiceError<S>> {
        match self {
            Self::Whole(limit) => match read_body(response, limit).await {
                Ok(bytes) => Ok((bytes, false)),
                Err(BodyError::TooBig(limit)) => Err(ServiceError::Response(format!(
                    "{}; narrow the request",
                    BodyError::TooBig(limit)
                ))),
                Err(error) => Err(error.into()),
            },
            Self::Tail(limit) => Ok(read_tail(response, limit).await?),
        }
    }
}

pub struct Answer {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub bytes: Vec<u8>,
    /// The start of the body was dropped, as `Read::Tail` does.
    pub cut: bool,
    spent: Option<u64>,
}

impl Answer {
    /// What this answer means if it was refused.
    pub fn refused<S: Service>(&self) -> ServiceError<S> {
        S::refused(self.status, &self.bytes, self.spent)
    }

    pub fn succeeded<S: Service>(self) -> Result<Self, ServiceError<S>> {
        if self.status.is_success() {
            Ok(self)
        } else {
            Err(self.refused())
        }
    }

    /// A JSON body, an empty one read as null.
    pub fn parsed<T: DeserializeOwned, S: Service>(&self) -> Result<T, ServiceError<S>> {
        Ok(parse(&self.bytes)?)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

/// The start of a body that is not in any shape a service's errors take, to quote in one.
pub fn quoted(body: &[u8]) -> Option<String> {
    let text: String = String::from_utf8_lossy(body).chars().take(400).collect();
    (!text.is_empty()).then_some(text)
}

/// A service whose calls name only a folder and a path: a session comes from
/// the folder, and from that its address and its credential.
pub trait Api: Service {
    type Session: Send + Sync;

    fn http() -> &'static Http<Self>;

    /// Who a call is from.
    fn session(
        data_dir: &Path,
    ) -> impl Future<Output = Result<Self::Session, ServiceError<Self>>> + Send;

    /// Each account has a rate limit of its own.
    fn account(session: &Self::Session) -> &str;

    /// A request to `path` with the session's address and credential.
    fn request(
        session: &Self::Session,
        client: &Client,
        method: Method,
        path: &str,
    ) -> RequestBuilder;

    /// The service turned the session's credential down, so what is held of it is dropped.
    fn turned_down(session: &Self::Session) -> impl Future<Output = ()> + Send;

    /// A session made after `turned_down` may have a new credential to try.
    fn renews(_session: &Self::Session) -> bool {
        false
    }
}

/// The calls of a service with an `Api`. The network half lives in functions
/// that are not generic over the answer, so each shape of answer only adds its
/// own parsing to the app rather than another copy of the request.
pub struct Calls<A>(PhantomData<fn() -> A>);

impl<A: Api> Default for Calls<A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: Api> Calls<A> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }

    /// One request as `session`. A refusal comes back as an answer like any other.
    pub async fn send_as(
        &self,
        session: &A::Session,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
        read: Read,
    ) -> Result<Answer, ServiceError<A>> {
        let http = A::http();
        let mut request = A::request(session, http.client()?, method, path);
        if !query.is_empty() {
            request = request.query(query);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let answer = http.exchange(A::account(session), request, read).await?;
        if answer.status == StatusCode::UNAUTHORIZED {
            A::turned_down(session).await;
        }
        Ok(answer)
    }

    /// One request as whoever the call is for, sent once more with a fresh
    /// session when the first credential turned out to be stale.
    pub async fn send(
        &self,
        data_dir: &Path,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
        read: Read,
    ) -> Result<Answer, ServiceError<A>> {
        let session = A::session(data_dir).await?;
        let answer = self
            .send_as(&session, method.clone(), path, query, body, read)
            .await?;
        if answer.status != StatusCode::UNAUTHORIZED || !A::renews(&session) {
            return Ok(answer);
        }
        let session = A::session(data_dir).await?;
        self.send_as(&session, method, path, query, body, read)
            .await
    }

    /// A successful answer in full.
    pub async fn fetch(
        &self,
        data_dir: &Path,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Answer, ServiceError<A>> {
        self.send(data_dir, method, path, query, body, Read::ALL)
            .await?
            .succeeded()
    }

    pub async fn get<T: DeserializeOwned>(
        &self,
        data_dir: &Path,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, ServiceError<A>> {
        self.fetch(data_dir, Method::GET, path, query, None)
            .await?
            .parsed()
    }

    /// Diffs, logs and files, which come back as plain text rather than JSON.
    pub async fn get_text(
        &self,
        data_dir: &Path,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<String, ServiceError<A>> {
        Ok(self
            .fetch(data_dir, Method::GET, path, query, None)
            .await?
            .text())
    }

    /// A write whose answer is what it made.
    pub async fn send_json<T: DeserializeOwned>(
        &self,
        data_dir: &Path,
        method: Method,
        path: &str,
        body: &Value,
    ) -> Result<T, ServiceError<A>> {
        self.fetch(data_dir, method, path, &[], Some(body))
            .await?
            .parsed()
    }

    /// A write whose answer nobody reads, such as approving or stopping something.
    pub async fn write(
        &self,
        data_dir: &Path,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(), ServiceError<A>> {
        self.fetch(data_dir, method, path, &[], body).await?;
        Ok(())
    }
}

#[derive(Debug)]
pub enum BodyError {
    Read(reqwest::Error),
    TooBig(usize),
}

impl fmt::Display for BodyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => error.fmt(formatter),
            Self::TooBig(limit) => write!(formatter, "more than {} MiB came back", limit >> 20),
        }
    }
}

impl<S: Service> From<BodyError> for ServiceError<S> {
    fn from(error: BodyError) -> Self {
        match error {
            BodyError::Read(error) => error.into(),
            BodyError::TooBig(_) => Self::Response(error.to_string()),
        }
    }
}

/// Reads an answer of up to `limit` bytes. Past that the read fails.
pub async fn read_body(response: Response, limit: usize) -> Result<Vec<u8>, BodyError> {
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err(BodyError::TooBig(limit));
    }
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(BodyError::Read)? {
        if bytes.len() + chunk.len() > limit {
            return Err(BodyError::TooBig(limit));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Reads the last `limit` bytes of an answer, such as the end of a long log,
/// and says whether its start was dropped.
pub async fn read_tail(response: Response, limit: usize) -> Result<(Vec<u8>, bool), BodyError> {
    let mut response = response;
    let mut bytes = Vec::new();
    let mut cut = false;
    while let Some(chunk) = response.chunk().await.map_err(BodyError::Read)? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > 2 * limit {
            bytes.drain(..bytes.len() - limit);
            cut = true;
        }
    }
    if bytes.len() > limit {
        bytes.drain(..bytes.len() - limit);
        cut = true;
    }
    Ok((bytes, cut))
}

/// Reads page after page, up to `max_pages`, until one says there is no next.
pub async fn collect_pages<C, T, E, F, Fut>(
    first: C,
    max_pages: u32,
    mut read: F,
) -> Result<Vec<T>, E>
where
    F: FnMut(C) -> Fut,
    Fut: Future<Output = Result<(Vec<T>, Option<C>), E>>,
{
    let mut all = Vec::new();
    let mut cursor = first;
    for _ in 0..max_pages.max(1) {
        let (mut items, next) = read(cursor).await?;
        all.append(&mut items);
        match next {
            Some(next) => cursor = next,
            None => break,
        }
    }
    Ok(all)
}

/// What a refused request means. `message` is what the service's body said,
/// when it said anything; `spent` is how long until a spent rate limit
/// resets, when the service says one is.
pub fn classify<S: Service>(
    status: StatusCode,
    message: Option<String>,
    spent: Option<u64>,
) -> ServiceError<S> {
    if let Some(resets_in_secs) = spent {
        return ServiceError::RateLimited {
            resets_in_secs: Some(resets_in_secs),
        };
    }
    let message = message.unwrap_or_else(|| {
        status
            .canonical_reason()
            .unwrap_or("the request failed")
            .to_string()
    });
    match status.as_u16() {
        401 => ServiceError::Auth(message),
        403 => ServiceError::Forbidden(message),
        404 => ServiceError::NotFound(message),
        429 => ServiceError::RateLimited {
            resets_in_secs: None,
        },
        status => ServiceError::Http { status, message },
    }
}

/// A JSON answer, an empty one read as null.
pub fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, serde_json::Error> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return serde_json::from_slice(b"null");
    }
    serde_json::from_slice(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_redirect_over_https_on_the_same_server_is_followed() -> Result<(), String> {
        let url = |raw: &str| Url::parse(raw).map_err(|error| error.to_string());
        let api = url("https://gitlab.acme.dev/api/v4/projects/a%2Fb")?;
        assert!(same_host(
            &url("https://gitlab.acme.dev/api/v4/projects/9")?,
            &api
        ));
        assert!(same_host(&url("https://gitlab.acme.dev:443/x")?, &api));
        assert!(!same_host(&url("http://gitlab.acme.dev/x")?, &api));
        assert!(!same_host(&url("https://gitlab.acme.dev:8443/x")?, &api));
        assert!(!same_host(&url("https://storage.acme.dev/x")?, &api));
        Ok(())
    }
}
