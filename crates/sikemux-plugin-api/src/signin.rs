//! Whether a plugin is signed in, as whom, and whether its sign-in still works,
//! in the one shape Settings reads for every plugin.

use serde::{Deserialize, Serialize};

use crate::service::{Service, ServiceError};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignInStatus<D> {
    pub configured: bool,
    pub ok: bool,
    /// The service refused the sign-in, so signing in again is what helps.
    pub auth_failed: bool,
    pub message: Option<String>,
    /// Who is signed in, as the person would name it, such as an email or a site.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_in_as: Option<String>,
    #[serde(flatten)]
    pub details: D,
}

/// What a plugin's sign-in details say of who is signed in, for its status to name.
pub trait SignedInAs {
    fn signed_in_as(&self) -> Option<String>;
}

impl<D: SignedInAs> SignInStatus<D> {
    /// What asking the service as the signed-in account found. A spent rate
    /// limit or a refusal of that one request still means the sign-in works,
    /// and a sign-in with its secret gone needs doing again.
    pub fn checked<T, S: Service>(
        configured: bool,
        details: D,
        check: &Result<T, ServiceError<S>>,
    ) -> Self {
        let (ok, auth_failed) = match check {
            Ok(_) | Err(ServiceError::RateLimited { .. } | ServiceError::Forbidden(_)) => {
                (true, false)
            }
            Err(ServiceError::Auth(_)) => (false, true),
            Err(ServiceError::Unconfigured) => (false, configured),
            Err(_) => (false, false),
        };
        let message = match check {
            Ok(_) => None,
            Err(ServiceError::Unconfigured) if !configured => None,
            Err(error) => Some(error.to_string()),
        };
        Self {
            configured,
            ok,
            auth_failed,
            message,
            signed_in_as: details.signed_in_as().filter(|_| configured),
            details,
        }
    }
}

/// `ankit in Acme, ankit in Side`, for a service with workspaces.
pub fn people_in<'a>(workspaces: impl Iterator<Item = (&'a str, &'a str)>) -> Option<String> {
    let named: Vec<String> = workspaces
        .map(|(person, workspace)| format!("{person} in {workspace}"))
        .collect();
    (!named.is_empty()).then(|| named.join(", "))
}
