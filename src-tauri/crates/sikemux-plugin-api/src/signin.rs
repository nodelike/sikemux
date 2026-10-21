//! Whether a plugin is signed in and its sign-in still works, in the shape
//! every plugin's sign-in view reads.

use serde::Serialize;

use crate::service::{Service, ServiceError};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignInStatus<D> {
    pub configured: bool,
    pub ok: bool,
    /// The service refused the sign-in, so signing in again is what helps.
    pub auth_failed: bool,
    pub message: Option<String>,
    #[serde(flatten)]
    pub details: D,
}

impl<D> SignInStatus<D> {
    /// What asking the service as the signed-in account found. A spent rate
    /// limit still means the sign-in works.
    pub fn checked<T, S: Service>(
        configured: bool,
        details: D,
        check: &Result<T, ServiceError<S>>,
    ) -> Self {
        let (ok, auth_failed) = match check {
            Ok(_) | Err(ServiceError::RateLimited { .. }) => (true, false),
            Err(ServiceError::Auth(_)) => (false, true),
            Err(_) => (false, false),
        };
        let message = match check {
            Ok(_) | Err(ServiceError::Unconfigured) => None,
            Err(error) => Some(error.to_string()),
        };
        Self {
            configured,
            ok,
            auth_failed,
            message,
            details,
        }
    }
}
