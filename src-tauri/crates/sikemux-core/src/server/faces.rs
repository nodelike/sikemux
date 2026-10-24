//! The face each agent wears in place of its CLI's logo: which pack, shape
//! and colour, worked out the same way for every client from the art's
//! catalogue, and the style the person picked. How a face moves follows the
//! agent's group, which clients already have.

use std::path::Path;
use std::sync::{Mutex, OnceLock};

use serde::Deserialize;
use sikemux_process::sync::Locked;

use super::kept::Kept;
use super::{CoreError, CoreResult};
use crate::protocol::{AgentFace, FaceStyle, WorkspaceAgent};

const FILE: &str = "face-style.json";
const EVERY_PACK: &str = "all";
const EVERY_SHAPE: &str = "mixed";
const MAX_ID_CHARS: usize = 40;

/// The art's catalogue without the art, generated from the Mac's packs.
#[derive(Deserialize)]
struct Catalogue {
    packs: Vec<Pack>,
}

#[derive(Deserialize)]
struct Pack {
    id: String,
    shapes: Vec<Shape>,
    colors: u32,
}

#[derive(Deserialize)]
struct Shape {
    id: String,
    #[serde(rename = "box")]
    view_box: String,
}

fn catalogue() -> &'static Catalogue {
    static CATALOGUE: OnceLock<Catalogue> = OnceLock::new();
    CATALOGUE.get_or_init(|| {
        serde_json::from_str(include_str!("../assets/faces.json"))
            .expect("the faces catalogue is generated and checked")
    })
}

/// FNV-1a over the id's UTF-16 units with a murmur finish, as the Mac's
/// catalogue hashes it, so every client lands on the same face.
fn hash(text: &str) -> u32 {
    let mut h: u32 = 2_166_136_261;
    for unit in text.encode_utf16() {
        h ^= u32::from(unit);
        h = h.wrapping_mul(16_777_619);
    }
    h ^= h >> 16;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h
}

/// The face `agent` wears under `style`, or none while faces are off and it
/// shows its logo.
fn face(style: &FaceStyle, agent: &WorkspaceAgent) -> Option<AgentFace> {
    if !style.on {
        return None;
    }
    let packs = &catalogue().packs;
    let first = packs.first()?;
    let h = hash(&agent.id);
    let own = agent.buddy.as_ref();
    let own_text = |key: &str| own.and_then(|buddy| buddy.get(key)?.as_str());
    let named = own_text("pack")
        .and_then(|id| packs.iter().find(|pack| pack.id == id))
        .and_then(|pack| {
            let id = own_text("shape")?;
            Some((pack, pack.shapes.iter().find(|shape| shape.id == id)?))
        });
    let (pack, shape) = named.or_else(|| {
        let every_pack = style.pack == EVERY_PACK;
        let pack = if every_pack {
            &packs[h as usize % packs.len()]
        } else {
            packs
                .iter()
                .find(|pack| pack.id == style.pack)
                .unwrap_or(first)
        };
        let shape = if every_pack || style.shape == EVERY_SHAPE {
            pack.shapes
                .get((h >> 8) as usize % pack.shapes.len().max(1))
        } else {
            pack.shapes
                .iter()
                .find(|shape| shape.id == style.shape)
                .or(pack.shapes.first())
        };
        Some((pack, shape?))
    })?;
    let colour = own
        .and_then(|buddy| buddy.get("color")?.as_u64())
        .map_or(h, |colour| colour as u32);
    Some(AgentFace {
        pack: pack.id.clone(),
        shape: shape.id.clone(),
        color: colour % pack.colors.max(1),
        view_box: shape.view_box.clone(),
    })
}

/// A change to the face style; a field left out stays as it is.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FaceStyleChange {
    on: Option<bool>,
    pack: Option<String>,
    shape: Option<String>,
    motion: Option<bool>,
}

pub(crate) struct Faces {
    style: Mutex<FaceStyle>,
    kept: Kept,
}

impl Faces {
    pub(crate) fn new(data_dir: Option<&Path>) -> Self {
        let kept = Kept::new(data_dir, FILE);
        Self {
            style: Mutex::new(kept.load()),
            kept,
        }
    }

    pub(crate) fn style(&self) -> FaceStyle {
        self.style.locked().clone()
    }

    pub(crate) fn set_style(&self, change: FaceStyleChange) -> CoreResult<FaceStyle> {
        let named = |id: &Option<String>| {
            id.as_ref()
                .is_none_or(|id| !id.trim().is_empty() && id.chars().count() <= MAX_ID_CHARS)
        };
        if !named(&change.pack) || !named(&change.shape) {
            return Err(CoreError::from("that is not a face pack or shape"));
        }
        let mut style = self.style.locked();
        if let Some(on) = change.on {
            style.on = on;
        }
        if let Some(pack) = change.pack {
            style.pack = pack;
        }
        if let Some(shape) = change.shape {
            style.shape = shape;
        }
        if let Some(motion) = change.motion {
            style.motion = motion;
        }
        self.kept.save(&*style);
        Ok(style.clone())
    }

    /// Puts on each agent the face it wears.
    pub(crate) fn fill(&self, agents: &mut [WorkspaceAgent]) {
        let style = self.style();
        for agent in agents {
            agent.face = face(&style, agent);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The numbers are src/buddies/catalogue.ts's `hash` of the same ids.
    #[test]
    fn hashes_agent_ids_as_the_macs_catalogue_does() {
        assert_eq!(hash(""), 1_572_119_809);
        assert_eq!(hash("agent-1"), 2_235_314_236);
        assert_eq!(hash("0192f4a8-7c1e-7b3a-9d2e-5f6a7b8c9d0e"), 1_043_506_299);
        assert_eq!(hash("ünï-🙂"), 4_157_272_692);
    }
}
