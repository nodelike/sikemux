use sikemux_plugin_api::{Service, ServiceError};

pub struct Forgejo;

impl Service for Forgejo {
    const ID: &'static str = "forgejo";
    const NAME: &'static str = "Forgejo";

    fn said(body: &[u8]) -> Option<String> {
        crate::client::error_message(body)
    }
}

pub type ForgejoError = ServiceError<Forgejo>;
pub type ForgejoResult<T> = Result<T, ForgejoError>;
