use sikemux_plugin_api::{Service, ServiceError};

pub struct Bitbucket;

impl Service for Bitbucket {
    const ID: &'static str = "bitbucket";
    const NAME: &'static str = "Bitbucket";

    fn said(body: &[u8]) -> Option<String> {
        crate::client::error_message(body)
    }
}

pub type BitbucketError = ServiceError<Bitbucket>;
pub type BitbucketResult<T> = Result<T, BitbucketError>;
