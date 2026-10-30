//! People's pictures beside their names. The window only draws images the app holds itself,
//! so each is fetched here and handed over as a `data:` address.

use std::path::Path;

use reqwest::{StatusCode, Url};
use serde::Deserialize;
use sikemux_plugin_api::images;

use crate::auth;
use crate::client;
use crate::error::{JiraError, JiraResult};

#[derive(Deserialize)]
pub struct ImageRef {
    pub url: String,
}

/// Avatars live on Gravatar's and Atlassian's public hosts, and Gravatar's image proxy at
/// `i0.wp.com` to `i3.wp.com`; nothing is sent with them.
fn allowed(url: &Url) -> bool {
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    url.host_str().is_some_and(|host| {
        host == "secure.gravatar.com"
            || matches!(host, "i0.wp.com" | "i1.wp.com" | "i2.wp.com" | "i3.wp.com")
            || host.ends_with(".atl-paas.net")
            || host.ends_with(".atlassian.com")
            || host.ends_with(".atlassian.net")
    })
}

#[derive(Deserialize)]
pub struct SiteImageRef {
    pub url: String,
    #[serde(default)]
    pub site: Option<String>,
}

/// The path of one of the site's own avatars, such as a project's, and nothing else of it.
fn site_avatar_path(url: &Url, host: &str) -> Option<String> {
    let path = url.path();
    (url.scheme() == "https"
        && url.host_str() == Some(host)
        && path.starts_with("/rest/api/")
        && path.contains("/avatar/")
        && !path.contains(".."))
    .then(|| path.to_string())
}

/// A project's avatar, which Jira serves only to someone signed in to the site.
pub async fn site_image(data_dir: &Path, input: SiteImageRef) -> JiraResult<String> {
    let refused = || JiraError::BadArg("that is not an avatar this site serves".into());
    let (site, credentials) = auth::credentials(data_dir, input.site.as_deref()).await?;
    let url = Url::parse(&input.url).map_err(|_| refused())?;
    let path = site_avatar_path(&url, &site.host).ok_or_else(refused)?;
    let query: Vec<(String, String)> = url.query_pairs().into_owned().collect();
    images::read_image(client::fetch(&credentials, &path, &query).await?).await
}

pub async fn image(input: ImageRef) -> JiraResult<String> {
    let refused = || JiraError::BadArg("that is not an avatar Jira serves".into());
    let url = Url::parse(&input.url).map_err(|_| refused())?;
    // Gravatar sends a person without a picture of their own on to Atlassian's drawn
    // initials, through its image proxy.
    let response = images::follow(&client::HTTP, url, move |url, request| async move {
        allowed(&url).then_some(request).ok_or_else(refused)
    })
    .await?;
    let status = response.status();
    if status != StatusCode::OK {
        return Err(JiraError::Http {
            status: status.as_u16(),
            message: "the avatar did not load".into(),
        });
    }
    images::read_image(response).await
}
