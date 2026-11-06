//! Plays a scene: its timing, the pointer and keys it sends, and what it records.

use super::*;

pub(super) enum Sink {
    Video {
        ffmpeg: Child,
        input: Option<ChildStdin>,
        partial: PathBuf,
        out: PathBuf,
    },
    Stills {
        frames: Vec<u64>,
    },
}

/// Where steps that last go, a little at a time each frame.
pub(super) enum Motion {
    Cursor {
        from: (f32, f32),
        control: (f32, f32),
        to: (f32, f32),
    },
    Camera {
        from: View,
        to: View,
    },
    Press {
        from: f32,
        to: f32,
    },
    Pinch {
        factor: f32,
    },
    Pan {
        dx: f32,
        dy: f32,
    },
    End,
}

pub(super) struct Tween {
    pub(super) id: u64,
    pub(super) start: f64,
    pub(super) duration: f64,
    pub(super) motion: Motion,
    pub(super) began: bool,
    pub(super) done: f64,
}

#[derive(Debug)]
pub(super) struct Finished;

impl std::fmt::Display for Finished {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("every still is taken")
    }
}

impl std::error::Error for Finished {}

pub(super) struct Director {
    pub(super) cx: HeadlessAppContext,
    pub(super) window: AnyWindowHandle,
    pub(super) stage: (f32, f32),
    pub(super) origin: (f32, f32),
    pub(super) pictures_a_frame: usize,
    pub(super) ticks: u64,
    pub(super) waited_ms: f64,
    pub(super) clock: PathBuf,
    pub(super) recording: bool,
    pub(super) recorded: u64,
    pub(super) pointer: Pointer,
    pub(super) held: bool,
    pub(super) sent: Option<(f32, f32)>,
    pub(super) forced: Option<Shape>,
    pub(super) view: View,
    pub(super) end: f32,
    pub(super) tweens: Vec<Tween>,
    pub(super) next_id: u64,
    pub(super) lens: Option<u64>,
    pub(super) bench: Option<gpui::Entity<workbench::Workbench>>,
    #[cfg(target_os = "macos")]
    pub(super) stand_in: Option<native::StandIn>,
    /// Who the rail shows as signed in, kept over whatever the core says.
    pub(super) signed_in: Option<sikemux_view::device::AccountInfo>,
    pub(super) compositor: Compositor,
    pub(super) output: (u32, u32),
    pub(super) sink: Sink,
    pub(super) still: Option<PathBuf>,
    pub(super) snap: Option<PathBuf>,
    pub(super) stills: PathBuf,
    pub(super) grid: bool,
    pub(super) events: Vec<Value>,
    pub(super) seed: u32,
    pub(super) began: Instant,
    /// Time spent drawing the app, reading its pictures back, framing them and encoding, for
    /// the progress line.
    pub(super) spent: [Duration; 4],
}

pub(super) fn hand_ease(p: f64) -> f64 {
    p * p * p * (10. + p * (6. * p - 15.))
}

pub(super) fn out_ease(p: f64) -> f64 {
    1. - (1. - p).powi(3)
}

pub(super) fn in_out(p: f64) -> f64 {
    if p < 0.5 {
        4. * p * p * p
    } else {
        1. - (-2. * p + 2.).powi(3) / 2.
    }
}

/// The camera's curve, CSS's cubic-bezier(0.2, 0.8, 0.2, 1): it starts fast and settles gently.
pub(super) fn lens_ease(p: f64) -> f64 {
    let along = |t: f64, a: f64, b: f64| {
        3. * a * t * (1. - t).powi(2) + 3. * b * t * t * (1. - t) + t.powi(3)
    };
    let (mut low, mut high) = (0., 1.);
    for _ in 0..30 {
        let mid = (low + high) / 2.;
        if along(mid, 0.2, 0.2) < p {
            low = mid;
        } else {
            high = mid;
        }
    }
    along((low + high) / 2., 0.8, 1.)
}

impl Director {
    pub(super) fn now(&self) -> f64 {
        self.ticks as f64 / (FPS * self.pictures_a_frame as f64)
    }

    pub(super) fn random(&mut self) -> f64 {
        self.seed = self
            .seed
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        self.seed as f64 / u32::MAX as f64
    }

    pub(super) fn mark(&mut self, kind: &str, detail: Option<Value>) {
        if !self.recording {
            return;
        }
        let at = (self.recorded as f64 / FPS * 1000.).round() / 1000.;
        let mut event = json!({ "kind": kind, "at": at });
        if let Some(detail) = detail {
            event["detail"] = detail;
        }
        self.events.push(event);
    }

    pub(super) fn tween(&mut self, duration: f64, motion: Motion) -> u64 {
        self.next_id += 1;
        let step = 1. / (FPS * self.pictures_a_frame as f64);
        self.tweens.push(Tween {
            id: self.next_id,
            start: self.now(),
            duration: duration.max(step),
            motion,
            began: false,
            done: 0.,
        });
        self.next_id
    }

    pub(super) fn until(&mut self, id: u64) -> anyhow::Result<()> {
        while self.tweens.iter().any(|tween| tween.id == id) {
            self.frame()?;
        }
        Ok(())
    }

    pub(super) fn wait(&mut self, seconds: f64) -> anyhow::Result<()> {
        for _ in 0..(seconds * FPS).round() as u64 {
            self.frame()?;
        }
        Ok(())
    }

    pub(super) fn dispatch(&mut self, event: PlatformInput) -> anyhow::Result<()> {
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_event(event, cx);
            })
            .map_err(|error| anyhow!("{error}"))
    }

    pub(super) fn position(&self) -> gpui::Point<gpui::Pixels> {
        point(px(self.pointer.x), px(self.pointer.y))
    }

    pub(super) fn write_clock(&self) {
        let ms = self.waited_ms + self.now() * 1000.;
        let fresh = self.clock.with_extension("next");
        if std::fs::write(&fresh, format!("{ms:.1}")).is_ok() {
            let _ = std::fs::rename(&fresh, &self.clock);
        }
    }

    /// Shows the world's account as signed in, over the core's view as each one arrives.
    pub(super) fn sign_in(&mut self) {
        let (Some(account), Some(bench)) = (self.signed_in.clone(), self.bench.clone()) else {
            return;
        };
        self.cx.update(|cx| {
            bench.update(cx, |bench, cx| {
                if let Some(view) = bench.feeds.this.view.as_mut() {
                    if view.account.as_deref() != Some(&account) {
                        view.account = Some(Box::new(account));
                        cx.notify();
                    }
                }
            })
        });
    }

    pub(super) fn apply(&mut self) -> anyhow::Result<()> {
        let now = self.now();
        let mut events = Vec::new();
        let position = self.position();
        let mut finished = Vec::new();
        for tween in &mut self.tweens {
            let p = ((now - tween.start) / tween.duration).clamp(0., 1.);
            let phase = if !tween.began {
                TouchPhase::Started
            } else if p >= 1. {
                TouchPhase::Ended
            } else {
                TouchPhase::Moved
            };
            match &tween.motion {
                Motion::Cursor { from, control, to } => {
                    let e = hand_ease(p) as f32;
                    self.pointer.x =
                        (1. - e).powi(2) * from.0 + 2. * (1. - e) * e * control.0 + e * e * to.0;
                    self.pointer.y =
                        (1. - e).powi(2) * from.1 + 2. * (1. - e) * e * control.1 + e * e * to.1;
                }
                Motion::Camera { from, to } => {
                    let e = lens_ease(p) as f32;
                    let width = from.width * (to.width / from.width).powf(e);
                    let along = if (to.width - from.width).abs() > 1. {
                        (from.width - width) / (from.width - to.width)
                    } else {
                        e
                    };
                    self.view = View {
                        x: from.x + (to.x - from.x) * along,
                        y: from.y + (to.y - from.y) * along,
                        width,
                    };
                }
                Motion::Press { from, to } => {
                    self.pointer.press = from + (to - from) * out_ease(p) as f32;
                }
                Motion::Pinch { factor } => {
                    let e = in_out(p);
                    let by = factor.powf((e - tween.done) as f32);
                    tween.done = e;
                    events.push(PlatformInput::Pinch(PinchEvent {
                        position,
                        delta: by - 1.,
                        modifiers: Modifiers::default(),
                        phase,
                    }));
                }
                Motion::Pan { dx, dy } => {
                    let e = in_out(p);
                    let part = (e - tween.done) as f32;
                    tween.done = e;
                    events.push(PlatformInput::Scroll(ScrollEvent {
                        position,
                        delta: ScrollDelta::Pixels(point(px(dx * part), px(dy * part))),
                        modifiers: Modifiers::default(),
                        touch_phase: phase,
                        source: ScrollSource::Pointer,
                    }));
                }
                Motion::End => {
                    self.end = p as f32;
                    self.pointer.opacity = (1. - p as f32 * 2.5).max(0.);
                }
            }
            tween.began = true;
            if p >= 1. {
                finished.push(tween.id);
            }
        }
        self.tweens.retain(|tween| !finished.contains(&tween.id));
        if self.lens.is_some_and(|lens| finished.contains(&lens)) {
            self.lens = None;
        }
        self.send_pointer()?;
        for event in events {
            self.dispatch(event)?;
        }
        Ok(())
    }

    pub(super) fn send_pointer(&mut self) -> anyhow::Result<()> {
        let now = (self.pointer.x, self.pointer.y);
        if self
            .sent
            .is_some_and(|sent| (sent.0 - now.0).abs() < 0.25 && (sent.1 - now.1).abs() < 0.25)
        {
            return Ok(());
        }
        self.sent = Some(now);
        let held = self.held.then_some(MouseButton::Left);
        self.dispatch(PlatformInput::MouseMove(MouseMoveEvent {
            position: self.position(),
            pressed_button: held,
            modifiers: Modifiers::default(),
        }))?;
        if self.forced.is_none() {
            let at = self.position();
            self.pointer.shape = self
                .cx
                .update_window(self.window, |_, window, _| {
                    gpui_base::test_support::snapshots(window)
                        .iter()
                        .filter(|element| element.visible() && element.bounds().contains(&at))
                        .filter_map(|element| {
                            let shape = match element.role()? {
                                Role::Button
                                | Role::Link
                                | Role::Tab
                                | Role::MenuItem
                                | Role::CheckBox
                                | Role::RadioButton
                                | Role::Switch
                                | Role::TreeItem
                                | Role::ListBoxOption => Shape::Hand,
                                Role::TextInput | Role::MultilineTextInput | Role::SearchInput => {
                                    Shape::Beam
                                }
                                _ => return None,
                            };
                            let size = element.bounds().size;
                            Some((f32::from(size.width) * f32::from(size.height), shape))
                        })
                        .min_by(|a, b| a.0.total_cmp(&b.0))
                        .map_or(Shape::Arrow, |(_, shape)| shape)
                })
                .map_err(|error| anyhow!("{error}"))?;
        }
        Ok(())
    }

    pub(super) fn pump(&mut self) {
        #[cfg(target_os = "macos")]
        native::run_loop();
        self.cx.update(|cx| cx.set_reduce_motion(false));
        self.cx.run_until_parked();
    }

    pub(super) fn frame(&mut self) -> anyhow::Result<()> {
        let wanted = self.snap.is_some()
            || self.recording
                && (self.still.is_some()
                    || match &self.sink {
                        Sink::Video { .. } => true,
                        Sink::Stills { frames } => frames.contains(&self.recorded),
                    });
        let step = Duration::from_secs_f64(1. / (FPS * self.pictures_a_frame as f64));
        for _ in 0..self.pictures_a_frame {
            let drawing = Instant::now();
            self.ticks += 1;
            self.write_clock();
            self.sign_in();
            self.apply()?;
            self.cx.advance_clock(step);
            self.pump();
            self.cx
                .update_window(self.window, |_, window, cx| {
                    window.simulate_next_frame(cx);
                })
                .map_err(|error| anyhow!("{error}"))?;
            self.pump();
            self.cx
                .update_window(self.window, |_, window, cx| window.draw(cx).clear(cx))
                .map_err(|error| anyhow!("{error}"))?;
            self.spent[0] += drawing.elapsed();
            if wanted {
                let reading = Instant::now();
                let shot = self.cx.capture_screenshot(self.window)?;
                self.spent[1] += reading.elapsed();
                if let Some(path) = self.snap.take() {
                    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
                    let mut shot = shot.clone();
                    #[cfg(target_os = "macos")]
                    if let Some(stand_in) = &self.stand_in {
                        stand_in.lay_over(&mut shot)?;
                    }
                    shot.save(&path)?;
                    println!("film: saved {}", path.display());
                }
                let framing = Instant::now();
                self.compositor.add(Picture {
                    window: &shot,
                    view: self.view,
                    pointer: Some(self.pointer),
                    end: self.end,
                    grid: self.grid,
                });
                self.spent[2] += framing.elapsed();
            }
        }
        if !self.recording {
            return Ok(());
        }
        if wanted {
            let (width, height) = self.output;
            let frame = self.compositor.take().to_vec();
            let still = self.still.take().or_else(|| match &self.sink {
                Sink::Stills { frames } if frames.contains(&self.recorded) => Some(
                    self.stills
                        .join(format!("still-{:.2}.png", self.recorded as f64 / FPS)),
                ),
                _ => None,
            });
            if let Some(path) = still {
                image::RgbaImage::from_raw(width, height, frame.clone())
                    .ok_or_else(|| anyhow!("frame size"))?
                    .save(&path)?;
                println!("film: saved {}", path.display());
            }
            if let Sink::Video {
                input: Some(input), ..
            } = &mut self.sink
            {
                let encoding = Instant::now();
                input.write_all(&frame)?;
                self.spent[3] += encoding.elapsed();
            }
        }
        self.recorded += 1;
        if self.recorded.is_multiple_of(60) {
            let each = |spent: Duration| spent.as_secs_f64() * 1000. / self.recorded as f64;
            println!(
                "film: {:.1} s filmed, {:.0} ms a frame: app {:.0}, reading {:.0}, framing {:.0}, encoding {:.0}",
                self.recorded as f64 / FPS,
                each(self.began.elapsed()),
                each(self.spent[0]),
                each(self.spent[1]),
                each(self.spent[2]),
                each(self.spent[3]),
            );
        }
        if let Sink::Stills { frames } = &self.sink {
            if frames.iter().all(|frame| *frame < self.recorded) {
                return Err(Finished.into());
            }
        }
        Ok(())
    }
}

/// A box on screen: left, top, width and height.
pub(super) type Rect = (f32, f32, f32, f32);

impl Director {
    pub(super) fn place(
        &mut self,
        step: &Step,
        at: usize,
    ) -> anyhow::Result<((f32, f32), Option<Rect>)> {
        let word = step
            .words
            .get(at)
            .ok_or_else(|| anyhow!("line {}: {} needs a place", step.line, step.verb))?
            .clone();
        if word.quoted {
            let label = word.text.clone();
            let found = self
                .cx
                .update_window(self.window, |_, window, _| {
                    gpui_base::test_support::snapshots(window)
                        .iter()
                        .filter(|element| {
                            element.visible() && element.label() == Some(label.as_str())
                        })
                        .map(|element| element.bounds())
                        .min_by(|a, b| {
                            (f32::from(a.size.width) * f32::from(a.size.height))
                                .total_cmp(&(f32::from(b.size.width) * f32::from(b.size.height)))
                        })
                })
                .map_err(|error| anyhow!("{error}"))?
                .ok_or_else(|| {
                    anyhow!(
                        "line {}: nothing labelled {:?} is shown",
                        step.line,
                        word.text
                    )
                })?;
            let rect = (
                f32::from(found.origin.x),
                f32::from(found.origin.y),
                f32::from(found.size.width),
                f32::from(found.size.height),
            );
            return Ok(((rect.0 + rect.2 / 2., rect.1 + rect.3 / 2.), Some(rect)));
        }
        match step.numbers(at).as_deref() {
            Some([x, y]) => Ok(((*x, *y), None)),
            Some([x, y, w, h]) => Ok(((x + w / 2., y + h / 2.), Some((*x, *y, *w, *h)))),
            _ => bail!("line {}: {:?} is not a place", step.line, word.text),
        }
    }

    pub(super) fn glide(
        &mut self,
        to: (f32, f32),
        duration: Option<f64>,
        background: bool,
    ) -> anyhow::Result<()> {
        let from = (self.pointer.x, self.pointer.y);
        let distance = ((to.0 - from.0).powi(2) + (to.1 - from.1).powi(2)).sqrt();
        if distance < 1. {
            return Ok(());
        }
        let duration =
            duration.unwrap_or((280. + 210. * (1. + distance as f64 / 70.).log2()) / 1000.);
        let bend = if self.random() < 0.5 { -1. } else { 1. } * (distance * 0.08).min(60.);
        let control = (
            (from.0 + to.0) / 2. + (to.1 - from.1) / distance * bend,
            (from.1 + to.1) / 2. - (to.0 - from.0) / distance * bend,
        );
        let id = self.tween(duration, Motion::Cursor { from, control, to });
        if !background {
            self.until(id)?;
        }
        Ok(())
    }

    pub(super) fn press_to(&mut self, to: f32, duration: f64) -> anyhow::Result<()> {
        let from = self.pointer.press;
        let id = self.tween(duration, Motion::Press { from, to });
        self.until(id)
    }

    pub(super) fn button(
        &mut self,
        down: bool,
        button: MouseButton,
        count: usize,
    ) -> anyhow::Result<()> {
        let position = self.position();
        let modifiers = Modifiers::default();
        self.dispatch(if down {
            PlatformInput::MouseDown(MouseDownEvent {
                button,
                position,
                modifiers,
                click_count: count,
                first_mouse: false,
            })
        } else {
            PlatformInput::MouseUp(MouseUpEvent {
                button,
                position,
                modifiers,
                click_count: count,
            })
        })
    }

    pub(super) fn click(&mut self, button: MouseButton, count: usize) -> anyhow::Result<()> {
        self.mark("click", None);
        self.press_to(1., 0.07)?;
        for at in 1..=count {
            self.button(true, button, at)?;
            self.wait(0.08)?;
            self.button(false, button, at)?;
            if at < count {
                self.wait(0.05)?;
            }
        }
        self.press_to(0., 0.13)?;
        self.wait(0.16)
    }

    pub(super) fn zoom(&mut self, to: View, duration: f64, background: bool) -> anyhow::Result<()> {
        let change = self.view.width / to.width;
        if change > 1.15 {
            self.mark(
                "zoom-in",
                Some(json!({ "by": (change * 100.).round() / 100. })),
            );
        } else if change < 1. / 1.15 {
            self.mark(
                "zoom-out",
                Some(json!({ "by": (100. / change).round() / 100. })),
            );
        }
        if let Some(lens) = self.lens.take() {
            self.tweens.retain(|tween| tween.id != lens);
        }
        let id = self.tween(
            duration,
            Motion::Camera {
                from: self.view,
                to,
            },
        );
        self.lens = Some(id);
        if !background {
            self.until(id)?;
        }
        Ok(())
    }

    /// The camera framing `centre` (window points) `scale` times closer, kept inside the stage.
    pub(super) fn view_at(&self, centre: (f32, f32), scale: f32) -> View {
        let (sw, sh) = self.stage;
        let width = sw / scale.clamp(1., 5.);
        let height = width * sh / sw;
        let (cx, cy) = (self.origin.0 + centre.0, self.origin.1 + centre.1);
        View {
            x: (cx - width / 2.).clamp(0., sw - width),
            y: (cy - height / 2.).clamp(0., sh - height),
            width,
        }
    }

    pub(super) fn type_text(&mut self, text: &str, per_second: f64) -> anyhow::Result<()> {
        for letter in text.chars() {
            self.mark("key", None);
            let keystroke = match letter {
                ' ' => Keystroke {
                    modifiers: Modifiers::default(),
                    key: "space".into(),
                    key_char: Some(" ".into()),
                },
                _ => Keystroke {
                    modifiers: Modifiers {
                        shift: letter.is_uppercase(),
                        ..Modifiers::default()
                    },
                    key: letter.to_lowercase().to_string(),
                    key_char: Some(letter.to_string()),
                },
            };
            self.cx
                .update_window(self.window, |_, window, cx| {
                    window.dispatch_keystroke(keystroke, cx);
                })
                .map_err(|error| anyhow!("{error}"))?;
            let gap = (0.55 + self.random() * 0.9) / per_second;
            self.wait(if letter == ' ' { gap * 1.3 } else { gap })?;
        }
        Ok(())
    }

    /// Lets the core and the window catch up for `seconds` of real time, unfilmed.
    pub(super) fn settle(&mut self, seconds: f64) -> anyhow::Result<()> {
        let started = Instant::now();
        let step = Duration::from_millis(50);
        while started.elapsed() < Duration::from_secs_f64(seconds) {
            self.pump();
            std::thread::sleep(step);
            self.waited_ms += 50.;
            self.write_clock();
            self.cx.advance_clock(step);
            self.cx
                .update_window(self.window, |_, window, cx| window.draw(cx).clear(cx))
                .map_err(|error| anyhow!("{error}"))?;
        }
        Ok(())
    }

    pub(super) fn run(&mut self, step: &Step) -> anyhow::Result<()> {
        let seconds = |at: usize| step.number(at).map(f64::from);
        let background = step.background;
        match step.verb.as_str() {
            "stage" | "window" | "replies" => {}
            "settle" => self.settle(seconds(0).unwrap_or(3.))?,
            "hold" => {
                let started = Instant::now();
                while started.elapsed() < Duration::from_secs_f64(seconds(0).unwrap_or(1.)) {
                    self.pump();
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            "record" => self.recording = true,
            "wait" => self.wait(seconds(0).unwrap_or(1.))?,
            "cursor" => {
                let (to, _) = self.place(step, 0)?;
                (self.pointer.x, self.pointer.y) = to;
            }
            "move" => {
                let (to, _) = self.place(step, 0)?;
                self.glide(to, seconds(1), background)?;
            }
            "click" | "dclick" | "rclick" => {
                if !step.words.is_empty() {
                    let (to, _) = self.place(step, 0)?;
                    self.glide(to, seconds(1), false)?;
                    self.wait(0.09)?;
                }
                let button = if step.verb == "rclick" {
                    MouseButton::Right
                } else {
                    MouseButton::Left
                };
                self.click(button, if step.verb == "dclick" { 2 } else { 1 })?;
            }
            "press" => {
                self.press_to(1., 0.07)?;
                self.held = true;
                self.button(true, MouseButton::Left, 1)?;
                self.wait(0.08)?;
            }
            "release" => {
                self.button(false, MouseButton::Left, 1)?;
                self.held = false;
                self.press_to(0., 0.13)?;
            }
            "drag" => {
                let (to, _) = self.place(step, 0)?;
                self.mark("drag", None);
                self.press_to(1., 0.07)?;
                self.held = true;
                self.button(true, MouseButton::Left, 1)?;
                self.wait(0.1)?;
                self.glide(to, seconds(1), false)?;
                self.wait(0.08)?;
                self.button(false, MouseButton::Left, 1)?;
                self.held = false;
                self.press_to(0., 0.13)?;
            }
            "type" => {
                let text = step.text(0)?.to_owned();
                self.type_text(&text, seconds(1).unwrap_or(15.))?;
            }
            "key" => {
                let name = step.text(0)?.to_owned();
                self.mark(if name == "enter" { "send" } else { "key" }, None);
                self.cx
                    .update_window(self.window, |_, window, cx| -> anyhow::Result<()> {
                        let keystroke = Keystroke::parse_with_os(&name, cx.platform_info().os)?;
                        window.dispatch_keystroke(keystroke, cx);
                        Ok(())
                    })
                    .map_err(|error| anyhow!("{error}"))??;
                self.wait(0.2)?;
            }
            "zoom" => {
                if step.text(0)? == "out" {
                    let full = View {
                        x: 0.,
                        y: 0.,
                        width: self.stage.0,
                    };
                    self.zoom(full, seconds(1).unwrap_or(0.7), background)?;
                } else {
                    let (centre, rect) = self.place(step, 0)?;
                    let (view, duration) =
                        match (rect, step.words.first().is_some_and(|word| word.quoted)) {
                            (Some(rect), false) => {
                                let pad = 48.;
                                let fit = (self.stage.0 / (rect.2 + pad * 2.))
                                    .min(self.stage.1 / (rect.3 + pad * 2.));
                                (self.view_at(centre, fit), seconds(1))
                            }
                            _ => (
                                self.view_at(centre, step.number(1).unwrap_or(1.6)),
                                seconds(2),
                            ),
                        };
                    self.zoom(view, duration.unwrap_or(0.7), background)?;
                }
            }
            "pinch" => {
                let factor = step.number(0).unwrap_or(1.5);
                self.mark("pinch", Some(json!({ "by": factor })));
                let id = self.tween(seconds(1).unwrap_or(0.9), Motion::Pinch { factor });
                if !background {
                    self.until(id)?;
                }
            }
            "pan" | "scroll" => {
                let (dx, dy) = if step.verb == "scroll" {
                    (0., step.number(0).unwrap_or(-300.))
                } else {
                    match step.numbers(0).as_deref() {
                        Some([dx, dy]) => (*dx, *dy),
                        _ => bail!("line {}: pan DX,DY", step.line),
                    }
                };
                self.mark("pan", None);
                let id = self.tween(seconds(1).unwrap_or(0.8), Motion::Pan { dx, dy });
                if !background {
                    self.until(id)?;
                }
            }
            "shape" => {
                self.forced = match step.text(0)? {
                    "arrow" => Some(Shape::Arrow),
                    "hand" => Some(Shape::Hand),
                    "beam" => Some(Shape::Beam),
                    _ => None,
                };
                if let Some(shape) = self.forced {
                    self.pointer.shape = shape;
                } else {
                    self.sent = None;
                }
            }
            "mark" => self.mark(step.text(0)?, None),
            "settings" => {
                let name = step.text(0)?;
                let page = sikemux_gpui::settings::Page::named(name)
                    .ok_or_else(|| anyhow!("line {}: no settings page called {name}", step.line))?;
                let bench = self.bench.clone().ok_or_else(|| anyhow!("no workbench"))?;
                self.cx
                    .update_window(self.window, |_, window, cx| {
                        bench.update(cx, |bench, cx| {
                            sikemux_gpui::settings::open(bench, page, window, cx)
                        })
                    })
                    .map_err(|error| anyhow!("{error}"))?;
                self.wait(0.3)?;
            }
            "still" => {
                self.still = Some(self.stills.join(format!("{}.png", step.text(0)?)));
                self.frame()?;
            }
            "snap" => {
                self.snap = Some(self.stills.join(format!("{}.png", step.text(0)?)));
                self.frame()?;
            }
            "end" => {
                self.mark("end-card", None);
                let full = View {
                    x: 0.,
                    y: 0.,
                    width: self.stage.0,
                };
                if self.view != full {
                    self.zoom(full, 0.9, true)?;
                }
                let id = self.tween(0.9, Motion::End);
                self.until(id)?;
                self.wait(seconds(0).unwrap_or(2.2))?;
            }
            other => bail!("line {}: no step called {other}", step.line),
        }
        Ok(())
    }
}
