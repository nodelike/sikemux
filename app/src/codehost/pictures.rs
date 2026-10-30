//! People's pictures on the code host, fetched through its plugin.

use gpui_kit::{AnyElement, App, Context};
use serde_json::json;

use super::HostArea;

impl HostArea {
    /// Someone's picture at `url`, or their initials until it arrives or when they have none.
    pub(super) fn face(
        &self,
        login: Option<&str>,
        url: Option<&str>,
        size: f32,
        cx: &App,
    ) -> AnyElement {
        self.pictures
            .face(login.unwrap_or("someone"), url, size, cx)
    }

    /// The signed-in account's picture.
    pub(super) fn avatar(&self) -> Option<&str> {
        let status = self.status.as_ref()?.as_ref().ok()?;
        status.details.avatar_url.as_deref()
    }

    /// Fetches the pictures the last frame drew without.
    pub(super) fn fetch_pictures(&self, cx: &mut Context<Self>) {
        self.pictures.fetch(
            |url| {
                let fetched = self.plugin("image", json!({ "url": url }));
                async move { fetched.await.ok()?.as_str().map(str::to_owned) }
            },
            |area| &area.pictures,
            cx,
        );
    }
}
