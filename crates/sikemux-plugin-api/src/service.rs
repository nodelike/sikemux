//! What can go wrong talking to an online service, said the same way by every
//! plugin that talks to one.

use std::convert::Infallible;
use std::fmt;
use std::marker::PhantomData;

use crate::network::{self, Unreachable};
use crate::{Category, PluginError};

/// The service a plugin talks to, named once for every message it gives.
pub trait Service: 'static {
    /// The lowercase name its messages start with, and the category of what only it says.
    const ID: &'static str;
    /// Its name as people know it.
    const NAME: &'static str;
    const SIGNED_OUT: &'static str = "not signed in";

    /// What to tell someone when the request never reached the service at `host`.
    fn unreachable(kind: Unreachable, _host: Option<&str>) -> String {
        network::unreachable_message(kind, Self::NAME)
    }
}

pub enum ServiceError<S> {
    Unconfigured,
    Auth(String),
    Forbidden(String),
    RateLimited {
        resets_in_secs: Option<u64>,
    },
    /// The service answered with an error of its own and this message.
    Api(String),
    Http {
        status: u16,
        message: String,
    },
    BadArg(String),
    NotFound(String),
    Unsupported(&'static str),
    /// The repository does not take this way of merging, though another may work.
    MergeMethod(String),
    Keychain(String),
    Transport(String),
    Unreachable {
        kind: Unreachable,
        host: Option<String>,
    },
    Response(String),
    #[doc(hidden)]
    Of(PhantomData<fn() -> S>, Infallible),
}

pub type ServiceResult<T, S> = Result<T, ServiceError<S>>;

impl<S: Service> fmt::Display for ServiceError<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let id = S::ID;
        match self {
            Self::Unconfigured => write!(formatter, "{id}: {}", S::SIGNED_OUT),
            Self::Auth(message) => write!(formatter, "{id}: sign-in failed: {message}"),
            Self::Forbidden(message) => write!(formatter, "{id}: not allowed: {message}"),
            Self::RateLimited {
                resets_in_secs: Some(secs),
            } => write!(
                formatter,
                "{id}: the rate limit is used up; try again in {}",
                wait_in_words(*secs)
            ),
            Self::RateLimited {
                resets_in_secs: None,
            } => write!(formatter, "{id}: too many requests; try again in a minute"),
            Self::Api(message) | Self::Transport(message) | Self::MergeMethod(message) => {
                write!(formatter, "{id}: {message}")
            }
            Self::Http { status, message } => write!(formatter, "{id}: http {status}: {message}"),
            Self::BadArg(message) | Self::NotFound(message) => formatter.write_str(message),
            Self::Unsupported(what) => write!(formatter, "{} cannot {what}", S::NAME),
            Self::Keychain(message) => write!(formatter, "keychain: {message}"),
            Self::Unreachable { kind, host } => {
                formatter.write_str(&S::unreachable(*kind, host.as_deref()))
            }
            Self::Response(message) => write!(formatter, "{id}: unexpected response: {message}"),
            Self::Of(_, never) => match *never {},
        }
    }
}

impl<S: Service> fmt::Debug for ServiceError<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl<S: Service> std::error::Error for ServiceError<S> {}

/// reqwest's own message stops at "error sending request"; why it could not
/// be sent, such as a DNS or certificate failure, is further down the chain.
impl<S: Service> From<reqwest::Error> for ServiceError<S> {
    fn from(error: reqwest::Error) -> Self {
        if let Some(kind) = network::unreachable(&error) {
            eprintln!("{}: {error}", S::ID);
            return Self::Unreachable {
                kind,
                host: error
                    .url()
                    .and_then(|url| url.host_str())
                    .map(str::to_owned),
            };
        }
        let mut message = error.to_string();
        let mut cause = std::error::Error::source(&error);
        while let Some(next) = cause {
            let text = next.to_string();
            if !message.contains(&text) {
                message.push_str(": ");
                message.push_str(&text);
            }
            cause = next.source();
        }
        Self::Transport(message)
    }
}

impl<S> From<serde_json::Error> for ServiceError<S> {
    fn from(error: serde_json::Error) -> Self {
        Self::Response(error.to_string())
    }
}

impl<S> From<std::io::Error> for ServiceError<S> {
    fn from(error: std::io::Error) -> Self {
        Self::Transport(error.to_string())
    }
}

impl<S> From<sikemux_keychain::KeychainError> for ServiceError<S> {
    fn from(error: sikemux_keychain::KeychainError) -> Self {
        match error {
            sikemux_keychain::KeychainError::Invalid(message) => Self::BadArg(message),
            sikemux_keychain::KeychainError::Failed(message) => Self::Keychain(message),
        }
    }
}

impl<S: Service> From<ServiceError<S>> for PluginError {
    fn from(error: ServiceError<S>) -> Self {
        let message = error.to_string();
        match error {
            ServiceError::Unconfigured => PluginError::new(Category::Unconfigured, message),
            ServiceError::Auth(_) => PluginError::new(Category::Auth, message),
            ServiceError::Forbidden(_) => PluginError::forbidden(message),
            ServiceError::RateLimited { .. } => {
                PluginError::new(Category::RateLimited, message).with_status(429)
            }
            ServiceError::Http { status, .. } => PluginError::http(status, message),
            ServiceError::BadArg(_) => PluginError::bad_params(message),
            ServiceError::NotFound(_) => {
                PluginError::new(Category::NotFound, message).with_status(404)
            }
            ServiceError::Unsupported(_) => PluginError::new(Category::Unsupported, message),
            ServiceError::MergeMethod(_) => PluginError::new(Category::MergeMethod, message),
            ServiceError::Keychain(_) => PluginError::new(Category::Keychain, message),
            ServiceError::Unreachable { kind, .. } => {
                PluginError::new(network::category(kind), message)
            }
            ServiceError::Api(_) | ServiceError::Transport(_) => {
                PluginError::new(Category::own(S::ID), message)
            }
            ServiceError::Response(_) => PluginError::new(Category::Response, message),
            ServiceError::Of(_, never) => match never {},
        }
    }
}

/// `40s`, `12 min` or `2 h 5 min`, for telling someone how long to wait.
pub fn wait_in_words(secs: u64) -> String {
    match secs {
        0..=59 => format!("{}s", secs.max(1)),
        60..=3599 => format!("{} min", secs.div_ceil(60)),
        _ => format!("{} h {} min", secs / 3600, (secs % 3600) / 60),
    }
}
