//! The pictures of the people in the history: the accounts the code host matches to its
//! latest commits' emails, read while someone is signed in to it.

use std::collections::HashMap;

use gpui_kit::{img, px, AnyElement, App, Context, IntoElement, Styled};
use serde_json::json;

use super::GitPane;
use crate::ui;

#[derive(Default)]
pub struct Authors {
    /// The host, repository and account the pictures were read for.
    read_for: Option<(String, String, Option<String>)>,
    /// Each author's picture address, by their email in lower case.
    by_email: HashMap<String, String>,
    pictures: crate::avatars::Avatars,
}

impl GitPane {
    /// Reads who wrote the host's latest commits, once for each repository and account.
    pub(super) fn read_authors(&mut self, cx: &mut Context<Self>) {
        let Some((host, account)) = self.host_of().filter(|_| self.signed_in) else {
            return;
        };
        let read_for = (host.host.clone(), host.repo.clone(), account.clone());
        if self.authors.read_for.as_ref() == Some(&read_for) {
            return;
        }
        self.authors.read_for = Some(read_for);
        self.authors.by_email.clear();
        let call = move |method: &'static str, more| {
            crate::codehost::call_of(&host.plugin, &host.repo, &account, method, more)
        };
        let listed = self
            .core
            .request("plugin", call("commitAuthors", json!({ "gitRef": null })));
        let core = self.core.clone();
        let image = move |url: &str| {
            let fetched = core.request("plugin", call("image", json!({ "url": url })));
            async move { fetched.await.ok()?["value"].as_str().map(str::to_owned) }
        };
        cx.spawn(async move |pane, cx| {
            let Ok(listed) = listed.await else {
                return;
            };
            let by_email: HashMap<String, String> = listed["value"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|author| {
                    let email = author["email"].as_str()?.to_lowercase();
                    Some((email, author["avatarUrl"].as_str()?.to_owned()))
                })
                .collect();
            pane.update(cx, |pane, cx| {
                for url in by_email.values() {
                    pane.authors.pictures.get(url);
                }
                pane.authors.by_email = by_email;
                pane.authors
                    .pictures
                    .fetch(image, |pane: &GitPane| &pane.authors.pictures, cx);
            })
            .ok();
        })
        .detach();
    }

    /// The author's picture on the code host, or their initials while it has none.
    pub(super) fn author_face(&self, name: &str, email: &str, size: f32, cx: &App) -> AnyElement {
        let picture = self
            .authors
            .by_email
            .get(&email.to_lowercase())
            .and_then(|url| self.authors.pictures.get(url));
        match picture {
            Some(image) => img(image)
                .size(px(size))
                .flex_none()
                .rounded_full()
                .into_any_element(),
            None => ui::initials(name, email, size, cx).into_any_element(),
        }
    }
}
