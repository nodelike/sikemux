//! Faces that move with what their agents are doing, as the Mac's do: one bobs while its agent
//! works and hops while it waits on the person. Each is a view of its own on the shared clock.

use std::sync::Arc;

use gpui::{
    div, img, px, AnyElement, App, Context, ElementId, Global, Image, IntoElement, ParentElement,
    Render, Styled, Window,
};
use sikemux_wire::protocol::{AgentFace, AgentGroup};

use super::clock::{self, Slot};

/// Up and down again, each way this long, rising this many of the face's own units.
const BOB_SECONDS: f32 = 0.7;
const BOB_LIFT: f32 = 0.8;
/// Up and back down in the first part of each hop, then resting for the rest of it.
const HOP_SECONDS: f32 = 1.2;
const HOP_LIFT: f32 = 1.8;
const HOP_PEAK: f32 = 0.25;
const HOP_LANDED: f32 = 0.55;

/// Whether faces in `group` move, as the person's face style and the system's ask for less
/// motion say.
pub fn moves(group: AgentGroup, cx: &App) -> bool {
    matches!(group, AgentGroup::Working | AgentGroup::Blocked)
        && !cx.reduce_motion()
        && cx.try_global::<Faces>().is_some_and(|faces| faces.on)
}

/// The face of agent `agent`, moving.
pub fn face(
    agent: &str,
    face: &AgentFace,
    group: AgentGroup,
    image: Arc<Image>,
    size: f32,
) -> AnyElement {
    Slot::new(
        ElementId::Name(format!("face-{agent}").into()),
        Face {
            image,
            size,
            side: side(&face.view_box),
            group,
        },
    )
    .into_any_element()
}

/// Turns the faces' motion on or off, as the person's face style says.
pub fn set_on(on: bool, cx: &mut App) {
    let faces = cx.default_global::<Faces>();
    if faces.on != on {
        faces.on = on;
        cx.refresh_windows();
    }
}

/// Whether the person's face style lets faces move.
#[derive(Default)]
struct Faces {
    on: bool,
}

impl Global for Faces {}

/// The side of a face's square, in the units its lift is measured in.
fn side(view_box: &str) -> f32 {
    view_box
        .split_whitespace()
        .nth(3)
        .and_then(|side| side.parse().ok())
        .filter(|side: &f32| *side > 0.)
        .unwrap_or(24.)
}

/// How far above its rest a face in `group` is, in its own units, `seconds` in.
fn lift(group: AgentGroup, seconds: f32) -> f32 {
    let ease = |at: f32| at * at * (3. - 2. * at);
    match group {
        AgentGroup::Working => {
            let at = (seconds / BOB_SECONDS) % 2.;
            BOB_LIFT * ease(if at < 1. { at } else { 2. - at })
        }
        AgentGroup::Blocked => {
            let at = (seconds / HOP_SECONDS) % 1.;
            if at < HOP_PEAK {
                HOP_LIFT * ease(at / HOP_PEAK)
            } else if at < HOP_LANDED {
                HOP_LIFT * ease(1. - (at - HOP_PEAK) / (HOP_LANDED - HOP_PEAK))
            } else {
                0.
            }
        }
        AgentGroup::Idle | AgentGroup::Done => 0.,
    }
}

struct Face {
    image: Arc<Image>,
    size: f32,
    side: f32,
    group: AgentGroup,
}

impl Render for Face {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rise = match clock::tick(window, cx) {
            true => lift(self.group, clock::seconds()) * self.size / self.side,
            false => 0.,
        };
        div().size(px(self.size)).flex_none().child(
            img(self.image.clone())
                .size(px(self.size))
                .relative()
                .top(px(-rise)),
        )
    }
}
