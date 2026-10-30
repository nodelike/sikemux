use sikemux_plugin_api::{Service, ServiceError};

pub struct Bitbucket;

impl Service for Bitbucket {
    const ID: &'static str = "bitbucket";
    const NAME: &'static str = "Bitbucket";
}

pub type BitbucketError = ServiceError<Bitbucket>;
pub type BitbucketResult<T> = Result<T, BitbucketError>;
