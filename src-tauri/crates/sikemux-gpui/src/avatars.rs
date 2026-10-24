//! People's pictures on a service, which its plugin fetches since the window may not reach
//! the service itself. The latest few hundred are kept; the rest are fetched again.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::Arc;

use gpui_kit::{img, px, AnyElement, App, Context, Image, IntoElement, Styled};

use crate::ui;

const KEPT: usize = 400;

enum Kept {
    Loading,
    Ready(Arc<Image>),
    Failed,
}

#[derive(Default)]
pub struct Avatars {
    kept: RefCell<HashMap<String, Kept>>,
    order: RefCell<VecDeque<String>>,
    /// Addresses drawn without a picture yet, fetched once the frame is drawn.
    wanted: RefCell<Vec<String>>,
}

impl Avatars {
    /// The picture at `url`, asked for after the frame when it is not here yet.
    pub fn get(&self, url: &str) -> Option<Arc<Image>> {
        match self.kept.borrow().get(url) {
            Some(Kept::Ready(image)) => return Some(image.clone()),
            Some(Kept::Loading | Kept::Failed) => return None,
            None => {}
        }
        self.wanted.borrow_mut().push(url.to_owned());
        None
    }

    /// Someone's picture at `url`, or their initials until it arrives or when they have none.
    pub fn face(&self, name: &str, url: Option<&str>, size: f32, cx: &App) -> AnyElement {
        match url.and_then(|url| self.get(url)) {
            Some(image) => img(image)
                .size(px(size))
                .flex_none()
                .rounded_full()
                .into_any_element(),
            None => ui::initials(name, "", size, cx).into_any_element(),
        }
    }

    fn keep(&self, url: String, kept: Kept) {
        let mut order = self.order.borrow_mut();
        let mut all = self.kept.borrow_mut();
        if all.insert(url.clone(), kept).is_none() {
            order.push_back(url);
        }
        while order.len() > KEPT {
            if let Some(oldest) = order.pop_front() {
                all.remove(&oldest);
            }
        }
    }

    /// Fetches the pictures the last frame drew without: `ask` answers an address with the
    /// picture as a `data:` address, and `avatars` finds this cache on the view again after.
    pub fn fetch<V: 'static, F>(
        &self,
        ask: impl Fn(&str) -> F,
        avatars: fn(&V) -> &Avatars,
        cx: &mut Context<V>,
    ) where
        F: Future<Output = Option<String>> + 'static,
    {
        let wanted: Vec<String> = self.wanted.borrow_mut().drain(..).collect();
        for url in wanted {
            if self.kept.borrow().contains_key(&url) {
                continue;
            }
            self.keep(url.clone(), Kept::Loading);
            let fetched = ask(&url);
            cx.spawn(async move |view, cx| {
                let image = fetched.await.and_then(|address| decoded(&address));
                view.update(cx, |view, cx| {
                    let ready = image.is_some();
                    avatars(view).keep(url, image.map_or(Kept::Failed, Kept::Ready));
                    if ready {
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
    }
}

/// The picture a `data:` address holds.
fn decoded(address: &str) -> Option<Arc<Image>> {
    let (mime, data) = address.strip_prefix("data:")?.split_once(";base64,")?;
    crate::picture::decode(mime, data)
}
