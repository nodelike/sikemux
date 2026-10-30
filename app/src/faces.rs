use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

use flate2::read::GzDecoder;
use gpui_kit::{App, Global, Image, ImageFormat};
use sikemux_wire::protocol::{AgentFace, AgentGroup};

include!(concat!(env!("OUT_DIR"), "/faces.rs"));

/// Every colour of a shape, each drawn in every state.
type Drawings = Vec<HashMap<String, String>>;

/// A pack of faces as the core's catalogue lists it, with what the person reads it as.
#[derive(serde::Deserialize)]
pub struct Pack {
    pub id: String,
    pub name: String,
    pub shapes: Vec<Shape>,
    /// How many colours each of its shapes comes in.
    pub colors: u32,
}

#[derive(serde::Deserialize)]
pub struct Shape {
    pub id: String,
    pub name: String,
    #[serde(rename = "box")]
    pub view_box: String,
}

/// Every pack, in the order the Mac lists them.
pub fn packs() -> &'static [Pack] {
    #[derive(serde::Deserialize)]
    struct Catalogue {
        packs: Vec<Pack>,
    }
    static PACKS: std::sync::LazyLock<Vec<Pack>> = std::sync::LazyLock::new(|| {
        serde_json::from_str::<Catalogue>(include_str!(
            "../../crates/sikemux-core/src/assets/faces.json"
        ))
        .map(|catalogue| catalogue.packs)
        .unwrap_or_default()
    });
    &PACKS
}

/// The shape of a pack in one of its colours, at rest.
pub fn resting(pack: &Pack, shape: &Shape, color: u32, cx: &App) -> Option<Arc<Image>> {
    let face = AgentFace {
        pack: pack.id.clone(),
        shape: shape.id.clone(),
        color: color % pack.colors.max(1),
        view_box: shape.view_box.clone(),
    };
    image(&face, AgentGroup::Idle, cx)
}

/// The faces drawn so far, and the shapes unpacked to draw them.
#[derive(Default)]
pub struct Faces {
    shapes: RefCell<HashMap<String, Option<Drawings>>>,
    images: RefCell<HashMap<(String, u32, String), Arc<Image>>>,
}

impl Global for Faces {}

pub fn install(cx: &mut App) {
    cx.set_global(Faces::default());
}

/// The face as a picture, drawn as its agent's group shows, cropped to the part of its square it fills.
pub fn image(face: &AgentFace, group: AgentGroup, cx: &App) -> Option<Arc<Image>> {
    let faces = cx.global::<Faces>();
    let shape = format!("{}.{}", face.pack, face.shape);
    let state = match group {
        AgentGroup::Blocked => "waiting",
        AgentGroup::Working => "working",
        AgentGroup::Done => "done",
        AgentGroup::Idle => "idle",
    }
    .to_owned();
    let key = (shape.clone(), face.color, state.clone());
    if let Some(image) = faces.images.borrow().get(&key) {
        return Some(image.clone());
    }
    let mut shapes = faces.shapes.borrow_mut();
    let drawings = shapes
        .entry(shape.clone())
        .or_insert_with(|| unpack(&shape))
        .as_ref()?;
    let drawing = drawings.get(face.color as usize)?.get(&state)?;
    let cropped = drawing.replacen(
        r#"viewBox="0 0 24 24""#,
        &format!(r#"viewBox="{}""#, face.view_box),
        1,
    );
    let image = Arc::new(Image::from_bytes(ImageFormat::Svg, cropped.into_bytes()));
    faces.images.borrow_mut().insert(key, image.clone());
    Some(image)
}

fn unpack(shape: &str) -> Option<Drawings> {
    let (_, packed) = DRAWINGS.iter().find(|(name, _)| *name == shape)?;
    let mut json = String::new();
    GzDecoder::new(*packed).read_to_string(&mut json).ok()?;
    serde_json::from_str(&json).ok()
}
