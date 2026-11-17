use sikemux_plugin_api::{Service, ServiceError};

pub struct Github;

impl Service for Github {
    const ID: &'static str = "github";
    const NAME: &'static str = "GitHub";

    fn said(body: &[u8]) -> Option<String> {
        crate::client::error_message(body)
    }
}

pub type GithubError = ServiceError<Github>;
pub type GithubResult<T> = Result<T, GithubError>;
