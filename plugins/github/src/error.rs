use sikemux_plugin_api::{Service, ServiceError};

pub struct Github;

impl Service for Github {
    const ID: &'static str = "github";
    const NAME: &'static str = "GitHub";
}

pub type GithubError = ServiceError<Github>;
pub type GithubResult<T> = Result<T, GithubError>;
