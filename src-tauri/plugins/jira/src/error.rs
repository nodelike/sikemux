use sikemux_plugin_api::{Service, ServiceError};

pub struct Jira;

impl Service for Jira {
    const ID: &'static str = "jira";
    const NAME: &'static str = "Jira";
    const SIGNED_OUT: &'static str = "no site is signed in";
}

pub type JiraError = ServiceError<Jira>;
pub type JiraResult<T> = Result<T, JiraError>;
