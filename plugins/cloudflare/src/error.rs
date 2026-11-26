use reqwest::StatusCode;
use sikemux_plugin_api::{Service, ServiceError};

use crate::client;

pub struct Cloudflare;

impl Service for Cloudflare {
    const ID: &'static str = "cloudflare";
    const NAME: &'static str = "Cloudflare";
    const SIGNED_OUT: &'static str = "no account is signed in";

    fn said(body: &[u8]) -> Option<String> {
        client::said(body)
    }

    fn refused(status: StatusCode, body: &[u8], spent: Option<u64>) -> ServiceError<Self> {
        client::refused(status, body, spent)
    }
}

pub type CloudflareError = ServiceError<Cloudflare>;
pub type CloudflareResult<T> = Result<T, CloudflareError>;
