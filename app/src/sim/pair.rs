//! Pairing an Android phone over Wi-Fi: a QR code its Wireless debugging scans, or the code it
//! shows typed in, then the phone joins the desk's device menu.

use std::sync::Arc;

use gpui::{fill, point, size, Bounds};
use gpui_kit::component::input::InputState;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    canvas, div, px, AppContext, Context, Entity, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Render, Styled, WeakEntity, Window,
};
use qrcodegen::{QrCode, QrCodeEcc};
use sikemux_simulator::android::pair::Invitation;
use sikemux_simulator::SimulatorManager;

use crate::theme::{
    Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_3, SPACE_4, TEXT_2XS, TEXT_SM, TEXT_XS,
};
use crate::ui::{self, Button, Field, Variant};
use crate::workbench::Workbench;

const QR_SIZE: f32 = 156.;

enum Stage {
    Scanning,
    Typing,
    Pairing,
    Paired(String),
    Failed(String),
}

pub struct PairPhone {
    bench: WeakEntity<Workbench>,
    engine: Arc<SimulatorManager>,
    agent: Option<String>,
    qr: Option<QrCode>,
    stage: Stage,
    address: Entity<InputState>,
    code: Entity<InputState>,
    waiting: Option<gpui_kit::Task<()>>,
}

impl PairPhone {
    fn new(
        bench: WeakEntity<Workbench>,
        engine: Arc<SimulatorManager>,
        agent: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let invitation = Invitation::default();
        let qr = QrCode::encode_text(&invitation.qr_text(), QrCodeEcc::Medium).ok();
        let mut pairing = Self {
            bench,
            engine,
            agent,
            qr,
            stage: Stage::Scanning,
            address: cx.new(|cx| InputState::new(window, cx).placeholder("192.168.1.24:37145")),
            code: cx.new(|cx| InputState::new(window, cx).placeholder("482913")),
            waiting: None,
        };
        pairing.wait_for_scan(invitation, cx);
        pairing
    }

    fn wait_for_scan(&mut self, invitation: Invitation, cx: &mut Context<Self>) {
        let engine = Arc::clone(&self.engine);
        self.waiting = Some(cx.spawn(async move |pairing, cx| {
            let paired =
                crate::desk::run(async move { engine.android().pair_scanned(&invitation).await })
                    .await;
            let _ = pairing.update(cx, |pairing, cx| pairing.heard(paired, cx));
        }));
    }

    fn pair_by_code(&mut self, cx: &mut Context<Self>) {
        let address = self.address.read(cx).value().trim().to_owned();
        let code = self.code.read(cx).value().trim().to_owned();
        if address.is_empty() || code.is_empty() {
            return;
        }
        self.stage = Stage::Pairing;
        let engine = Arc::clone(&self.engine);
        self.waiting = Some(cx.spawn(async move |pairing, cx| {
            let paired =
                crate::desk::run(async move { engine.android().pair(&address, &code).await }).await;
            let _ = pairing.update(cx, |pairing, cx| pairing.heard(paired, cx));
        }));
        cx.notify();
    }

    fn heard(&mut self, paired: Option<Result<String, String>>, cx: &mut Context<Self>) {
        self.stage = match paired {
            Some(Ok(name)) => Stage::Paired(name),
            Some(Err(error)) => Stage::Failed(error),
            None => return,
        };
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.waiting = None;
        let _ = self.bench.update(cx, |bench, cx| bench.close_pairing(cx));
    }

    fn show(&mut self, name: String, cx: &mut Context<Self>) {
        let agent = self.agent.clone();
        let _ = self.bench.update(cx, |bench, cx| {
            if let Some(agent) = agent {
                bench
                    .sims
                    .update(cx, |sims, cx| sims.show_when_listed(&agent, &name, cx));
            }
            bench.close_pairing(cx);
        });
    }
}

/// The QR code, dark modules on white, as a phone's camera reads it.
fn qr_picture(code: &QrCode) -> impl IntoElement {
    let side = code.size();
    let dark: Vec<(i32, i32)> = (0..side)
        .flat_map(|y| (0..side).map(move |x| (x, y)))
        .filter(|&(x, y)| code.get_module(x, y))
        .collect();
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let module = f32::from(bounds.size.width) / side as f32;
            for &(x, y) in &dark {
                window.paint_quad(fill(
                    Bounds::new(
                        point(
                            bounds.origin.x + px(x as f32 * module),
                            bounds.origin.y + px(y as f32 * module),
                        ),
                        size(px(module.ceil()), px(module.ceil())),
                    ),
                    gpui_kit::black(),
                ));
            }
        },
    )
    .size(px(QR_SIZE))
}

impl Render for PairPhone {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let line =
            |text: String| ui::text(text, TEXT_XS, FontWeight::NORMAL).text_color(t.text_secondary);
        let quiet =
            |text: String| ui::text(text, TEXT_2XS, FontWeight::NORMAL).text_color(t.text_tertiary);
        let mut body = div().flex().flex_col().gap(SPACE_2);
        let mut actions = div().flex().justify_end().gap(SPACE_1H);
        let cancel = Button::new("pair-cancel", "Cancel", Variant::Outline)
            .on_click(cx.listener(|pairing, _, _, cx| pairing.close(cx)));
        match &self.stage {
            Stage::Scanning => {
                body = body
                    .children(self.qr.as_ref().map(|code| {
                        div()
                            .self_center()
                            .p(SPACE_2)
                            .rounded(px(8.))
                            .bg(gpui_kit::white())
                            .child(qr_picture(code))
                    }))
                    .child(line(
                        "1. On the phone: Settings › Developer options › Wireless debugging".into(),
                    ))
                    .child(line(
                        "2. Tap “Pair device with QR code” and scan this".into(),
                    ))
                    .child(quiet("Waiting for your phone…".into()))
                    .child(
                        div()
                            .text_size(crate::theme::ui_text(TEXT_XS))
                            .text_color(t.accent)
                            .cursor_pointer()
                            .on_mouse_down(
                                gpui_kit::MouseButton::Left,
                                cx.listener(|pairing, _, _, cx| {
                                    pairing.waiting = None;
                                    pairing.stage = Stage::Typing;
                                    cx.notify();
                                }),
                            )
                            .child("Use a pairing code instead"),
                    );
                actions = actions.child(cancel);
            }
            Stage::Typing | Stage::Pairing => {
                let pairing = matches!(self.stage, Stage::Pairing);
                body = body
                    .child(line(
                        "On the phone: Wireless debugging › Pair device with pairing code.".into(),
                    ))
                    .child(quiet("IP address and port".into()))
                    .child(Field::new(&self.address).mono().small())
                    .child(quiet("Wi-Fi pairing code".into()))
                    .child(Field::new(&self.code).mono().small());
                actions = actions.child(cancel).child(
                    Button::new(
                        "pair-go",
                        if pairing { "Pairing…" } else { "Pair" },
                        Variant::Primary,
                    )
                    .when(!pairing, |button| {
                        button.on_click(cx.listener(|pairing, _, _, cx| pairing.pair_by_code(cx)))
                    }),
                );
            }
            Stage::Paired(name) => {
                body = body
                    .child(line(format!("Paired with {name}.")))
                    .child(quiet(
                        "It shows in the device menu while Wireless debugging is on.".into(),
                    ));
                let name = name.clone();
                actions = actions.child(
                    Button::new("pair-show", "Show it on the desk", Variant::Primary).on_click(
                        cx.listener(move |pairing, _, _, cx| pairing.show(name.clone(), cx)),
                    ),
                );
            }
            Stage::Failed(error) => {
                body = body.child(line(error.clone()));
                actions = actions.child(cancel).child(
                    Button::new("pair-again", "Use a pairing code", Variant::Outline).on_click(
                        cx.listener(|pairing, _, _, cx| {
                            pairing.stage = Stage::Typing;
                            cx.notify();
                        }),
                    ),
                );
            }
        }
        div()
            .w(px(320.))
            .p(SPACE_4)
            .flex()
            .flex_col()
            .gap(SPACE_3)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_1)
                    .child(
                        ui::text("Pair a phone over Wi-Fi", TEXT_SM, FontWeight::SEMIBOLD)
                            .text_color(t.text_primary),
                    )
                    .child(quiet(
                        "The phone and this computer must be on the same network.".into(),
                    )),
            )
            .child(body)
            .child(actions)
    }
}

/// Opens the pairing dialog; a phone it pairs can then be shown on `agent`'s desk.
pub fn open(
    bench: &mut Workbench,
    agent: Option<String>,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let weak = cx.entity().downgrade();
    let engine = Arc::clone(bench.sims.read(cx).engine());
    let pairing = cx.new(|cx| PairPhone::new(weak, engine, agent, window, cx));
    bench.show_pairing(pairing, cx);
}
