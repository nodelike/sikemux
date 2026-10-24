use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

use flate2::read::GzDecoder;
use gpui_kit::{App, Global, Image, ImageFormat};
use serde::Deserialize;

/// The app's file icons and the tables it tells a file's language by, written by
/// scripts/generate-file-icons.mjs from the same source as the app's.
static BUNDLE: &[u8] = include_bytes!("../assets/file-icons.json.gz");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Bundle {
    file: String,
    names: HashMap<String, String>,
    extensions: HashMap<String, String>,
    languages: HashMap<String, String>,
    language_names: HashMap<String, String>,
    language_extensions: HashMap<String, String>,
    known: HashMap<String, String>,
    icons: HashMap<String, String>,
}

impl Bundle {
    /// The language a file is written in, as the app's `languageOf` names it.
    fn language_of(&self, name: &str) -> Option<&str> {
        let name = name.to_lowercase();
        let name = name.rsplit(['/', '\\']).next().unwrap_or_default();
        let extension = name
            .rfind('.')
            .filter(|dot| *dot > 0)
            .map_or("", |dot| &name[dot + 1..]);
        self.language_names
            .get(name)
            .map(String::as_str)
            .or_else(|| name.starts_with("dockerfile").then_some("docker"))
            .or_else(|| name.starts_with(".env.").then_some("dotenv"))
            .or_else(|| self.known.get(name).map(String::as_str))
            .or_else(|| self.language_extensions.get(extension).map(String::as_str))
            .or_else(|| {
                (!extension.is_empty())
                    .then(|| self.known.get(extension).map(String::as_str))
                    .flatten()
            })
    }

    /// The icon a file shows, picked as the app's `fileIconOf` picks it: by its whole name,
    /// then by its longest extension, then by its language.
    fn icon_of(&self, name: &str) -> &str {
        let lower = name.to_lowercase();
        if let Some(icon) = self.names.get(&lower) {
            return icon;
        }
        for (dot, _) in lower.match_indices('.').filter(|(dot, _)| *dot > 0) {
            if let Some(icon) = self.extensions.get(&lower[dot + 1..]) {
                return icon;
            }
        }
        self.language_of(name)
            .and_then(|language| self.languages.get(language))
            .unwrap_or(&self.file)
    }
}

/// The bundle once a file icon was first drawn, and each icon drawn so far.
#[derive(Default)]
pub struct FileIcons {
    bundle: OnceCell<Option<Bundle>>,
    images: RefCell<HashMap<String, Arc<Image>>>,
}

impl Global for FileIcons {}

pub fn install(cx: &mut App) {
    cx.set_global(FileIcons::default());
}

fn bundle(icons: &FileIcons) -> Option<&Bundle> {
    icons
        .bundle
        .get_or_init(|| {
            let mut json = String::new();
            GzDecoder::new(BUNDLE).read_to_string(&mut json).ok()?;
            serde_json::from_str(&json).ok()
        })
        .as_ref()
}

/// The language a file is written in, by its name or path.
pub fn language(name: &str, cx: &App) -> Option<String> {
    bundle(cx.try_global::<FileIcons>()?)?
        .language_of(name)
        .map(str::to_owned)
}

pub fn icon(name: &str, cx: &App) -> Option<Arc<Image>> {
    let icons = cx.try_global::<FileIcons>()?;
    let bundle = bundle(icons)?;
    drawn(icons, bundle, bundle.icon_of(name))
}

/// The bundle's icon called `icon`, such as `nodejs`.
pub fn named(icon: &str, cx: &App) -> Option<Arc<Image>> {
    let icons = cx.try_global::<FileIcons>()?;
    drawn(icons, bundle(icons)?, icon)
}

fn drawn(icons: &FileIcons, bundle: &Bundle, icon: &str) -> Option<Arc<Image>> {
    if let Some(image) = icons.images.borrow().get(icon) {
        return Some(image.clone());
    }
    let svg = bundle.icons.get(icon)?;
    let image = Arc::new(Image::from_bytes(
        ImageFormat::Svg,
        svg.clone().into_bytes(),
    ));
    icons
        .images
        .borrow_mut()
        .insert(icon.to_owned(), image.clone());
    Some(image)
}
