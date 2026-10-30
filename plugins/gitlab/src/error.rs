use sikemux_plugin_api::{Service, ServiceError};

pub struct Gitlab;

impl Service for Gitlab {
    const ID: &'static str = "gitlab";
    const NAME: &'static str = "GitLab";
}

pub type GitlabError = ServiceError<Gitlab>;
pub type GitlabResult<T> = Result<T, GitlabError>;
