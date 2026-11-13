//! The dialog that picks one agent's face, as the Mac's buddy picker does: a pack, a shape in
//! it and a colour, or back to the default the face style in Settings gives every agent.

use crate::ui::Clicks;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, img, px, AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled,
};
use sikemux_view::workspace::{AgentFace, WorkspaceAgent};

use crate::faces::{self, Pack};
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_3, SPACE_3H, SPACE_5, TEXT_2XS,
    TEXT_SM, TEXT_XS,
};
use crate::ui::{self, dialog};
use crate::workbench::Workbench;
use sikemux_calls::call;
use sikemux_calls::workspace::AgentPatch;
use sikemux_calls::workspace::UpdateAgentParams;
use sikemux_view::workspace::AgentBuddy;

/// Which agent's face is being picked, and the pack whose shapes it shows.
pub struct BuddyPicker {
    agent: String,
    pack: String,
}

impl Workbench {
    pub fn open_buddy_picker(&mut self, agent: &str, cx: &mut Context<Self>) {
        let Some(face) = self
            .workspace_agent(agent)
            .and_then(|agent| agent.face.clone())
        else {
            return;
        };
        self.overlays.buddy_picker = Some(BuddyPicker {
            agent: agent.to_owned(),
            pack: face.pack,
        });
        cx.notify();
    }

    fn workspace_agent(&self, id: &str) -> Option<&WorkspaceAgent> {
        self.shown()?
            .workspace
            .agents
            .iter()
            .find(|agent| agent.id == id)
    }

    fn close_buddy_picker(&mut self, cx: &mut Context<Self>) {
        self.overlays.buddy_picker = None;
        cx.notify();
    }

    fn set_buddy(&self, agent: &str, buddy: Option<AgentBuddy>) {
        self.target()
            .call_typed::<call::UpdateAgent>(UpdateAgentParams::new(
                agent,
                AgentPatch {
                    buddy: Some(buddy),
                    ..AgentPatch::default()
                },
            ));
    }
}

/// What the agent picked for itself: its pack and shape, and its colour, each when it has one.
fn own(agent: &WorkspaceAgent) -> (Option<&str>, Option<u64>) {
    let buddy = agent.buddy.as_ref();
    (
        buddy.and_then(|buddy| buddy.pack.as_deref()),
        buddy.and_then(|buddy| buddy.color.map(u64::from)),
    )
}

/// The dialog, while it is open.
pub fn view(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let picking = bench.overlays.buddy_picker.as_ref()?;
    let agent = bench.workspace_agent(&picking.agent)?;
    let face = agent.face.clone()?;
    let packs = faces::packs();
    let pack = packs
        .iter()
        .find(|pack| pack.id == picking.pack)
        .or_else(|| packs.iter().find(|pack| pack.id == face.pack))?;
    let t = Tokens::get(cx).clone();
    let (own_pack, own_color) = own(agent);
    let chosen = own_pack.is_some();
    let colors = pack.colors.max(1);
    let color = face.color % colors;
    let named = packs
        .iter()
        .find(|known| known.id == face.pack)
        .and_then(|known| {
            let shape = known.shapes.iter().find(|shape| shape.id == face.shape)?;
            Some(format!("{} · {}", known.name, shape.name))
        })
        .unwrap_or_default();
    let drawn = |pack: &Pack, shape: &str, color: u32, size: f32| {
        let face = AgentFace {
            pack: pack.id.clone(),
            shape: shape.to_owned(),
            color,
            view_box: pack
                .shapes
                .iter()
                .find(|known| known.id == shape)
                .map(|known| known.view_box.clone())
                .unwrap_or_default(),
        };
        faces::image(&face, sikemux_wire::protocol::AgentGroup::Idle, cx)
            .map(|image| img(image).size(px(size)).flex_none().into_any_element())
            .unwrap_or_else(|| div().size(px(size)).flex_none().into_any_element())
    };
    let tile = |id: SharedString, on: bool, size: f32| {
        div()
            .id(id)
            .size(px(size))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .rounded(theme::RADIUS_2)
            .border_1()
            .cursor_pointer()
            .border_color(if on {
                gpui::transparent_black()
            } else {
                t.border
            })
            .when(on, |tile| tile.bg(t.surface_chosen))
            .when(!on, |tile| tile.hover(|style| style.bg(t.surface_hover)))
    };
    let agent_id = picking.agent.clone();
    let head_shape = if face.pack == pack.id {
        face.shape.clone()
    } else {
        pack.shapes
            .first()
            .map(|shape| shape.id.clone())
            .unwrap_or_default()
    };
    let swatches = div()
        .flex()
        .flex_wrap()
        .justify_end()
        .gap(SPACE_1)
        .children((0..colors).map(|index| {
            let (agent_id, own_pack) = (agent_id.clone(), own_pack.map(str::to_owned));
            let shape = face.shape.clone();
            tile(
                SharedString::from(format!("buddy-color-{index}")),
                index == color,
                26.,
            )
            .child(drawn(pack, &head_shape, index, 20.))
            .clicks(cx.listener(move |bench, _, _, _| {
                let buddy = match &own_pack {
                    Some(own_pack) => AgentBuddy {
                        pack: Some(own_pack.clone()),
                        shape: Some(shape.clone()),
                        color: Some(index),
                    },
                    None => AgentBuddy {
                        color: Some(index),
                        ..AgentBuddy::default()
                    },
                };
                bench.set_buddy(&agent_id, Some(buddy));
            }))
        }));
    let head = div()
        .flex()
        .items_center()
        .gap(SPACE_3)
        .child(drawn(
            packs
                .iter()
                .find(|known| known.id == face.pack)
                .unwrap_or(pack),
            &face.shape,
            face.color,
            44.,
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(SPACE_0)
                .min_w_0()
                .child(
                    ui::text(
                        agent.name.clone().unwrap_or_else(|| t!("Buddy").into()),
                        TEXT_SM,
                        FontWeight::SEMIBOLD,
                    )
                    .text_color(t.text_primary),
                )
                .child(
                    div()
                        .text_size(crate::theme::ui_text(TEXT_2XS))
                        .text_color(t.text_tertiary)
                        .child(named),
                ),
        )
        .child(div().flex_1().min_w_0().child(swatches));
    let pack_list =
        div()
            .id("buddy-packs")
            .w(px(150.))
            .flex_none()
            .flex()
            .flex_col()
            .gap(SPACE_0)
            .overflow_y_scroll()
            .children(packs.iter().map(|candidate| {
                let on = candidate.id == pack.id;
                let picked = candidate.id.clone();
                div()
                    .id(SharedString::from(format!("buddy-pack-{}", candidate.id)))
                    .h(px(28.))
                    .px(SPACE_2)
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(SPACE_2)
                    .rounded(theme::RADIUS_2)
                    .cursor_pointer()
                    .text_size(crate::theme::ui_text(TEXT_XS))
                    .text_color(if on { t.text_primary } else { t.text_secondary })
                    .when(on, |row| row.bg(t.surface_chosen))
                    .when(!on, |row| row.hover(|style| style.bg(t.surface_hover)))
                    .children(candidate.shapes.first().map(|shape| {
                        drawn(candidate, &shape.id, color % candidate.colors.max(1), 18.)
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(candidate.name.clone()),
                    )
                    .when(candidate.id == face.pack, |row| {
                        row.child(ui::icon(Icon::Check, 12.).text_color(t.text_secondary))
                    })
                    .clicks(cx.listener(move |bench, _, _, cx| {
                        if let Some(picking) = bench.overlays.buddy_picker.as_mut() {
                            picking.pack = picked.clone();
                        }
                        cx.notify();
                    }))
            }));
    let grid = div()
        .id("buddy-shapes")
        .flex_1()
        .min_w_0()
        .flex()
        .flex_wrap()
        .content_start()
        .gap(SPACE_1H)
        .overflow_y_scroll()
        .children(pack.shapes.iter().map(|shape| {
            let on = face.pack == pack.id && face.shape == shape.id;
            let (agent_id, pack_id, shape_id) =
                (agent_id.clone(), pack.id.clone(), shape.id.clone());
            tile(
                SharedString::from(format!("buddy-shape-{}", shape.id)),
                on,
                52.,
            )
            .tooltip(ui::tooltip(shape.name.clone()))
            .child(drawn(pack, &shape.id, color, 40.))
            .clicks(cx.listener(move |bench, _, _, _| {
                bench.set_buddy(
                    &agent_id,
                    Some(AgentBuddy {
                        pack: Some(pack_id.clone()),
                        shape: Some(shape_id.clone()),
                        color: own_color.map(|color| color as u32),
                    }),
                );
            }))
        }));
    let body = div()
        .px(SPACE_5)
        .py(SPACE_3H)
        .flex()
        .flex_col()
        .gap(SPACE_3H)
        .child(head)
        .child(
            div()
                .h(px(300.))
                .flex()
                .gap(SPACE_3)
                .child(pack_list)
                .child(grid),
        )
        .into_any_element();
    let this = cx.entity();
    let close: dialog::Answer =
        Rc::new(move |_, cx| this.update(cx, |bench, cx| bench.close_buddy_picker(cx)));
    let hint = if chosen {
        t!("Picked for this agent")
    } else {
        t!("Following the default in Settings")
    };
    let reset: dialog::Answer = {
        let (this, agent_id) = (cx.entity(), agent_id);
        Rc::new(move |_, cx| {
            this.update(cx, |bench, _| {
                let buddy = own_color.map(|color| AgentBuddy {
                    color: Some(color as u32),
                    ..AgentBuddy::default()
                });
                bench.set_buddy(&agent_id, buddy);
            })
        })
    };
    let foot = dialog::hinted_foot(hint, cx)
        .when(chosen, |foot| {
            foot.child(dialog::button(
                "buddy-default",
                t!("Use Default").into(),
                None,
                reset,
                cx,
            ))
        })
        .child(dialog::button(
            "buddy-done",
            t!("Done").into(),
            Some(t.accent),
            close.clone(),
            cx,
        ));
    Some(
        dialog::sheet_of(
            px(600.),
            t!("Change buddy").into(),
            body,
            foot,
            close.clone(),
            close,
            cx,
        )
        .into_any_element(),
    )
}
