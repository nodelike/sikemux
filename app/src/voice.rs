//! Dictation: the mic in a chat's composer, or right ⌥ held on its own anywhere, speaks
//! into the chat or terminal holding the keyboard. `sikemux-voice` runs the helper; this
//! keeps how far it has got and where the words go.

use std::f32::consts::TAU;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::WindowExt;
use gpui_kit::{
    div, Animation, AnimationExt, App, Context, Entity, EventEmitter, IntoElement, ParentElement,
    Render, SharedString, Styled, Subscription, Task, Window,
};
use sikemux_voice::{Reason, Stage, Voice, VoiceEvent};
use sikemux_wire::protocol::NetworkChange;
use tokio::sync::mpsc;

/// What clicking a chat's mic does.
pub type Act = Rc<dyn Fn(&mut Window, &mut App)>;

/// How long the listening mic takes to fade and come back, as today's app pulses it.
const PULSE: Duration = Duration::from_millis(1400);

/// Right ⌥ also begins shortcuts, so the mic waits to see it held on its own.
const START_DELAY: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Off,
    Unsupported,
    Preparing,
    Ready,
    Listening,
    Transcribing,
}

/// Where the words being spoken go.
#[derive(Clone, PartialEq)]
pub enum Target {
    Chat(String),
    Terminal(crate::terminal::TermKey),
}

/// How a chat's mic looks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mic {
    Hidden,
    Idle,
    Listening,
    Writing,
}

/// What the window does with what dictation heard or has to say.
pub enum Heard {
    /// The words spoken, for where they were going, or for the clipboard with nowhere.
    Words(Option<Target>, String),
    Say(SharedString),
}

#[derive(Default)]
pub struct Dictation {
    voice: Option<Voice>,
    phase: Phase,
    /// How much of the model is down, while it downloads.
    downloading: Option<f64>,
    /// Why dictation cannot run here, or why it last stopped.
    reason: Option<SharedString>,
    /// Going offline paused the model's download, which starts again once it is back.
    waiting_for_network: bool,
    target: Option<Target>,
    partial: SharedString,
    recording: bool,
    held: bool,
    starting: Option<Task<()>>,
    /// The chat whose message goes once the words are in, and whether it steers.
    send_after: Option<(String, bool)>,
    _heard: Option<Task<()>>,
}

impl EventEmitter<Heard> for Dictation {}

impl Dictation {
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// How much of the speech model is down, while it downloads.
    pub fn downloading(&self) -> Option<f64> {
        self.downloading
    }

    pub fn reason(&self) -> Option<&SharedString> {
        self.reason.as_ref()
    }

    pub fn network_changed(&mut self, change: NetworkChange, cx: &mut Context<Self>) {
        if change == NetworkChange::Offline || !self.waiting_for_network {
            return;
        }
        self.waiting_for_network = false;
        self.prepare(cx);
    }

    /// Turns dictation on or off as the person's preference says.
    pub fn follow(&mut self, on: bool, cx: &mut Context<Self>) {
        match (on, self.voice.is_some()) {
            (true, false) => {
                let Some(dir) = models_dir() else {
                    return;
                };
                let (events, mut heard) = mpsc::unbounded_channel();
                self.voice = Some(Voice::new(dir, events));
                self._heard = Some(cx.spawn(async move |this, cx| {
                    while let Some(event) = heard.recv().await {
                        if this.update(cx, |this, cx| this.heard(&event, cx)).is_err() {
                            break;
                        }
                    }
                }));
                self.prepare(cx);
            }
            (false, true) => {
                if let Some(voice) = self.voice.take() {
                    voice.shutdown();
                }
                *self = Dictation::default();
                cx.notify();
            }
            _ => {}
        }
    }

    /// Stops the helper and the model's download, as the app quits.
    pub fn shutdown(&mut self) {
        if let Some(voice) = &self.voice {
            voice.shutdown();
        }
    }

    fn prepare(&mut self, cx: &mut Context<Self>) {
        let Some(voice) = &self.voice else {
            return;
        };
        let status = voice.status();
        if !status.supported {
            self.phase = Phase::Unsupported;
            self.reason = status.reason.map(SharedString::from);
            return cx.notify();
        }
        (self.phase, self.reason) = (Phase::Preparing, None);
        self.downloading = (!status.installed).then_some(0.);
        if let Err(error) = voice.prepare() {
            (self.phase, self.reason) = (Phase::Off, Some(error.into()));
        }
        cx.notify();
    }

    fn heard(&mut self, event: &VoiceEvent, cx: &mut Context<Self>) {
        match event {
            VoiceEvent::Progress { stage, fraction } => {
                self.phase = Phase::Preparing;
                self.downloading = (*stage == Stage::Download).then_some(*fraction);
            }
            VoiceEvent::Ready => {
                (self.phase, self.downloading, self.reason) = (Phase::Ready, None, None)
            }
            VoiceEvent::Listening if self.phase == Phase::Ready => {
                (self.phase, self.partial) = (Phase::Listening, SharedString::default());
            }
            VoiceEvent::Partial { text }
                if matches!(self.phase, Phase::Listening | Phase::Transcribing) =>
            {
                self.partial = text.clone().into();
            }
            VoiceEvent::Transcript { text } => {
                self.phase = Phase::Ready;
                self.deliver(text.clone(), cx);
            }
            VoiceEvent::Cancelled => self.settle(Phase::Ready),
            VoiceEvent::Exited => {
                self.settle(Phase::Off);
                self.reason =
                    Some("The voice helper stopped. Hold the key again to restart it.".into());
            }
            VoiceEvent::Error { reason, message } => {
                let message: SharedString = message.clone().into();
                let lost = reason.stops_dictation();
                self.settle(if lost { Phase::Off } else { Phase::Ready });
                if lost {
                    (self.downloading, self.reason) = (None, Some(message.clone()));
                }
                if *reason == Reason::Offline {
                    self.waiting_for_network = true;
                }
                cx.emit(Heard::Say(message));
            }
            VoiceEvent::Listening | VoiceEvent::Partial { .. } => return,
        }
        cx.notify();
    }

    /// Back to `phase` with nothing on its way.
    fn settle(&mut self, phase: Phase) {
        (self.phase, self.recording) = (phase, false);
        (self.target, self.partial, self.send_after) = (None, SharedString::default(), None);
    }

    fn deliver(&mut self, text: String, cx: &mut Context<Self>) {
        let target = self.target.take();
        self.partial = SharedString::default();
        if !text.is_empty() {
            cx.emit(Heard::Words(target, text));
        }
    }

    /// Takes the chat whose message waits on the words just delivered into it.
    pub fn take_send_after(&mut self, agent: &str) -> Option<bool> {
        let (waiting, steer) = self.send_after.take()?;
        (waiting == agent).then_some(steer)
    }

    fn start_when_ready(&mut self, cx: &mut Context<Self>) {
        let Some(voice) = &self.voice else {
            return;
        };
        match self.phase {
            Phase::Ready => {
                self.recording = true;
                if let Err(error) = voice.start() {
                    self.settle(Phase::Ready);
                    cx.emit(Heard::Say(error.into()));
                }
            }
            Phase::Off => {
                self.target = None;
                cx.emit(Heard::Say(
                    "Loading the speech model. Try again in a moment.".into(),
                ));
                self.prepare(cx);
            }
            Phase::Preparing => {
                self.target = None;
                let what = match self.downloading {
                    Some(fraction) => format!("downloading ({}%)", (fraction * 100.).round()),
                    None => "loading".to_owned(),
                };
                cx.emit(Heard::Say(
                    format!("The speech model is still {what}.").into(),
                ));
            }
            _ => self.target = None,
        }
        cx.notify();
    }

    /// The mic in `agent`'s composer: the first click listens, the next types what was said.
    /// Whether dictation was off, so the window turns it on.
    pub fn toggle(&mut self, agent: &str, cx: &mut Context<Self>) -> bool {
        if self.voice.is_none() {
            return true;
        }
        if self.recording {
            self.finish(cx);
        } else if self.starting.is_none() && self.phase != Phase::Transcribing {
            self.target = Some(Target::Chat(agent.to_owned()));
            self.start_when_ready(cx);
        }
        false
    }

    /// Right ⌥ went down on its own, with the keyboard in `target`. Not while the last
    /// recording is still being written out, which goes where that one was started.
    pub fn begin_hold(&mut self, target: Option<Target>, cx: &mut Context<Self>) {
        if self.voice.is_none()
            || self.starting.is_some()
            || self.recording
            || self.phase == Phase::Transcribing
        {
            return;
        }
        (self.held, self.target) = (true, target);
        self.starting = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(START_DELAY).await;
            this.update(cx, |this, cx| {
                this.starting = None;
                this.start_when_ready(cx);
            })
            .ok();
        }));
    }

    pub fn end_hold(&mut self, cx: &mut Context<Self>) {
        if !self.held {
            return;
        }
        if self.starting.is_some() {
            return self.abandon_hold(cx);
        }
        self.held = false;
        if self.recording {
            self.finish(cx);
        }
    }

    /// Another key went with right ⌥, so it was a shortcut rather than speech; or the window
    /// lost the keyboard mid-hold. Dictation the mic started goes on.
    pub fn abandon_hold(&mut self, cx: &mut Context<Self>) {
        if !self.held {
            return;
        }
        self.held = false;
        if self.starting.take().is_some() {
            self.target = None;
        }
        if !self.recording {
            return;
        }
        self.recording = false;
        (self.target, self.partial) = (None, SharedString::default());
        if let Some(voice) = &self.voice {
            let _ = voice.cancel();
        }
        cx.notify();
    }

    pub fn holding(&self) -> bool {
        self.held
    }

    fn finish(&mut self, cx: &mut Context<Self>) {
        self.recording = false;
        self.phase = Phase::Transcribing;
        if let Some(voice) = &self.voice {
            let _ = voice.stop();
        }
        cx.notify();
    }

    /// Stops dictating into `agent`'s chat so what was said goes into its draft, and has
    /// its message sent once it has; whether words are on their way.
    pub fn send_after_words(&mut self, agent: &str, steer: bool, cx: &mut Context<Self>) -> bool {
        if !self.writes_into(agent) {
            return false;
        }
        if self.recording {
            self.finish(cx);
        }
        self.send_after = Some((agent.to_owned(), steer));
        true
    }

    fn writes_into(&self, agent: &str) -> bool {
        matches!(&self.target, Some(Target::Chat(into)) if into == agent)
            && (self.recording || self.phase == Phase::Transcribing)
    }

    pub fn mic(&self, agent: &str) -> Mic {
        if self.phase == Phase::Unsupported || !cfg!(target_os = "macos") {
            return Mic::Hidden;
        }
        match self.phase {
            Phase::Listening if self.writes_into(agent) => Mic::Listening,
            Phase::Transcribing if self.writes_into(agent) => Mic::Writing,
            _ => Mic::Idle,
        }
    }

    /// What has been heard so far for `agent`'s chat, while it is still being spoken.
    pub fn heard_for(&self, agent: &str) -> Option<SharedString> {
        (self.writes_into(agent) && !self.partial.is_empty()).then(|| self.partial.clone())
    }
}

fn models_dir() -> Option<PathBuf> {
    let home = sikemux_process::paths::home()?;
    Some(
        home.join("Library/Application Support")
            .join(crate::core::IDENTIFIER)
            .join("voice"),
    )
}

impl crate::workbench::Workbench {
    /// Right ⌥ held on its own starts dictation into what holds the keyboard, and letting
    /// go types what was said; any other modifier with it makes it a shortcut.
    pub fn voice_keys(
        &mut self,
        event: &gpui_kit::ModifiersChangedEvent,
        window: &mut gpui_kit::Window,
        cx: &mut Context<Self>,
    ) {
        let sides = window.modifier_sides();
        let modifiers = event.modifiers;
        let alone = sides.right.alt
            && !sides.left.alt
            && !modifiers.control
            && !modifiers.shift
            && !modifiers.platform;
        let holding = self.voice.read(cx).holding();
        if alone && !holding {
            let target = self.typing_into(window, cx);
            self.voice
                .update(cx, |voice, cx| voice.begin_hold(target, cx));
        } else if holding && !modifiers.alt {
            self.voice.update(cx, |voice, cx| voice.end_hold(cx));
        } else if holding && !alone {
            self.voice.update(cx, |voice, cx| voice.abandon_hold(cx));
        }
    }

    /// What holds the keyboard for dictation: a terminal, or a chat's draft.
    fn typing_into(&self, window: &gpui_kit::Window, cx: &gpui_kit::App) -> Option<Target> {
        if let Some(id) = crate::terminal::focused(self) {
            return Some(Target::Terminal(id));
        }
        self.chats.open.iter().find_map(|(agent, chat)| {
            let draft = &chat.composer.as_ref()?.draft;
            let focused = gpui_kit::Focusable::focus_handle(draft.read(cx), cx).is_focused(window);
            focused.then(|| Target::Chat(agent.clone()))
        })
    }

    pub fn dictated(
        &mut self,
        heard: &Heard,
        window: &mut gpui_kit::Window,
        cx: &mut Context<Self>,
    ) {
        match heard {
            Heard::Say(said) => window.push_notification(said.clone(), cx),
            Heard::Words(Some(Target::Chat(agent)), text) => {
                let Some(draft) = self
                    .chats
                    .open
                    .get(agent)
                    .and_then(|chat| Some(chat.composer.as_ref()?.draft.clone()))
                else {
                    return self.copy_dictation(text, window, cx);
                };
                draft.update(cx, |draft, cx| {
                    let caret = draft.selected_range().start;
                    let spaced = draft.value()[..caret]
                        .chars()
                        .next_back()
                        .is_none_or(char::is_whitespace);
                    let words = if spaced {
                        text.clone()
                    } else {
                        format!(" {text}")
                    };
                    draft.replace(words, window, cx);
                });
                let steer = self
                    .voice
                    .update(cx, |voice, _| voice.take_send_after(agent));
                if let Some(steer) = steer {
                    self.send(agent, steer, window, cx);
                }
            }
            Heard::Words(Some(Target::Terminal(id)), text) => {
                if !crate::terminal::type_text(self, *id, text.clone(), cx) {
                    self.copy_dictation(text, window, cx);
                }
            }
            Heard::Words(None, text) => self.copy_dictation(text, window, cx),
        }
    }

    fn copy_dictation(&self, text: &str, window: &mut gpui_kit::Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text.to_owned()));
        window.push_notification(
            "No agent or terminal had focus, so the dictation was copied to the clipboard.",
            cx,
        );
    }
}

/// A chat's mic, a view of its own so its pulse while listening redraws it alone rather than
/// the transcript.
pub struct MicButton {
    voice: Entity<Dictation>,
    agent: String,
    toggle: Act,
    _heard: Subscription,
}

impl MicButton {
    pub fn new(
        voice: Entity<Dictation>,
        agent: String,
        toggle: Act,
        cx: &mut Context<Self>,
    ) -> Self {
        let heard = cx.observe(&voice, |_, _, cx| cx.notify());
        MicButton {
            voice,
            agent,
            toggle,
            _heard: heard,
        }
    }
}

impl Render for MicButton {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mic = self.voice.read(cx).mic(&self.agent);
        let label = match mic {
            Mic::Hidden => return div().into_any_element(),
            Mic::Listening => "Stop and type what you said",
            Mic::Writing => "Writing what you said",
            Mic::Idle => "Dictate — or hold right ⌥ anywhere",
        };
        let toggle = self.toggle.clone();
        let danger = crate::theme::Tokens::get(cx).danger;
        let button = crate::ui::IconButton::new("composer-dictate", crate::icons::Icon::Mic, label)
            .glyph(16.)
            .width(31.)
            .disabled(mic == Mic::Writing)
            .on_click(move |_, window, cx| toggle(window, cx));
        if mic != Mic::Listening {
            return button.into_any_element();
        }
        div()
            .child(button.pressed(true).tint(danger))
            .with_animation(
                "composer-dictate-pulse",
                Animation::new(PULSE).repeat().with_max_fps(30.),
                |mic, delta| mic.opacity(1. - 0.45 * (1. - (delta * TAU).cos()) / 2.),
            )
            .into_any_element()
    }
}
