//! The HTTP client every plugin talks to its service with: kept warm, with
//! the same deadlines everywhere, a cap on requests at once, and a cap on how
//! much one answer may be.

use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::OnceLock;
use std::time::Duration;

use reqwest::{Client, ClientBuilder, RequestBuilder, Response, Url};
use tokio::sync::Semaphore;

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

/// One warm client for a service, and the bound on its requests in flight,
/// which every screen of the plugin shares.
pub struct Http<S> {
    client: OnceLock<Option<Client>>,
    redirects: Redirects,
    deadline: Deadline,
    permits: Semaphore,
    service: PhantomData<fn() -> S>,
}

impl<S: Service> Http<S> {
    pub const fn new(redirects: Redirects, deadline: Deadline, in_flight: usize) -> Self {
        Self {
            client: OnceLock::new(),
            redirects,
            deadline,
            permits: Semaphore::const_new(in_flight),
            service: PhantomData,
        }
    }

    pub fn client(&self) -> Result<&Client, ServiceError<S>> {
        self.client
            .get_or_init(|| builder::<S>(self.redirects, self.deadline).build().ok())
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
