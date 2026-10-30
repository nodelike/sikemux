//! How a chat's rows look: its markdown styles at the chat's text size, and the theme's tints.

use super::*;

/// How the chat's markdown looks: an answer, a thought, and what the person wrote.
pub(super) struct Looks {
    /// The prose of replies, of thoughts and of the person's messages, at the size they
    /// were designed, and at the chat's text size.
    pub(super) designed: Styles,
    pub(super) styles: RefCell<Styles>,
    pub(super) scale: Cell<f32>,
    pub(super) line: Hsla,
    pub(super) bubble: Hsla,
    pub(super) quiet: Hsla,
    pub(super) faint: Hsla,
    pub(super) ink: Hsla,
    pub(super) hover: Hsla,
    pub(super) rule: Hsla,
    pub(super) accent: Hsla,
    pub(super) provider: String,
    pub(super) reader: Entity<Reading>,
    pub(super) tints: Tints,
    pub(super) bench: WeakEntity<Workbench>,
    pub(super) agent: String,
    /// The pictures in the chat's replies, read from their bytes once, by part.
    pub(super) pictures: RefCell<HashMap<SharedString, Option<Picture>>>,
    /// What the chat's host holds of the names its messages write.
    pub(super) files: Rc<RefCell<files::Files>>,
}

#[derive(Clone)]
pub(super) struct Styles {
    pub(super) text: Rc<Style>,
    pub(super) thought: Rc<Style>,
    pub(super) user: Rc<Style>,
}

impl Looks {
    /// The agent's mark as the rail draws it, its face when it wears one, else its CLI's.
    pub(super) fn mark_of(&self, agent: &str, cli: &str, size: f32, cx: &App) -> AnyElement {
        let row = (self.bench.upgrade()).and_then(|bench| bench.read(cx).agent_by_id(agent));
        match row {
            Some(row) => row.mark(size, cx),
            None => ui::mark(cli, size),
        }
    }

    /// Sets the prose to `scale` times its designed size, and answers the replies' style.
    pub(super) fn rescale(&self, scale: f32) -> Rc<Style> {
        let scaled = |style: &Rc<Style>| Rc::new(style.scaled(scale));
        let styles = Styles {
            text: scaled(&self.designed.text),
            thought: scaled(&self.designed.thought),
            user: scaled(&self.designed.user),
        };
        let text = styles.text.clone();
        *self.styles.borrow_mut() = styles;
        self.scale.set(scale);
        text
    }
}

/// The colours a run of tool calls is drawn in, mixed once.
pub(super) struct Tints {
    pub(super) looked: Hsla,
    pub(super) changed: Hsla,
    pub(super) deleted: Hsla,
    pub(super) ran: Hsla,
    pub(super) called: Hsla,
    pub(super) failed: Hsla,
    pub(super) added: Hsla,
    pub(super) deleted_line: Hsla,
    pub(super) added_ground: Hsla,
    pub(super) deleted_ground: Hsla,
    pub(super) added_mark: Hsla,
    pub(super) deleted_mark: Hsla,
    pub(super) spine: Hsla,
    pub(super) tick: Hsla,
    pub(super) line_soft: Hsla,
    pub(super) well: Hsla,
    pub(super) composer: Hsla,
}

impl Tints {
    pub(super) fn new(t: &Tokens) -> Tints {
        let dim = t.text_tertiary;
        Tints {
            looked: theme::mix(t.git_renamed, dim, 0.76),
            changed: theme::mix(t.git_modified, dim, 0.76),
            deleted: theme::mix(t.git_deleted, dim, 0.76),
            ran: theme::mix(t.cmd, dim, 0.7),
            called: theme::mix(t.live, dim, 0.66),
            failed: t.danger,
            added: t.git_added,
            deleted_line: t.git_deleted,
            added_ground: Hsla {
                a: 0.09,
                ..t.git_added
            },
            deleted_ground: Hsla {
                a: 0.09,
                ..t.git_deleted
            },
            added_mark: Hsla {
                a: 0.26,
                ..t.git_added
            },
            deleted_mark: Hsla {
                a: 0.26,
                ..t.git_deleted
            },
            spine: t.tree_spine,
            tick: t.tree_tick,
            line_soft: t.line_soft,
            well: Hsla {
                a: 0.62,
                ..t.surface_sunken
            },
            composer: t.surface_composer,
        }
    }
}

/// Colours a code block once syntax.rs has worked it out: the first draw asks for it,
/// and the chat redraws when it arrives.
pub(super) fn code_colours(reader: Entity<Reading>) -> CodeColours {
    let reader = reader.downgrade();
    let coloured: syntax::BlockColoured = Rc::new(move |cx| {
        let _ = reader.update(cx, |_, cx| cx.notify());
    });
    Rc::new(move |lang, code, cx| syntax::block_runs(&lang?.to_lowercase(), code, &coloured, cx))
}

impl Looks {
    /// What reaching for a file the chat names does, once its host says the file is there.
    pub(super) fn file_pressed(&self) -> files::FilePressed {
        let (bench, agent, files) = (self.bench.clone(), self.agent.clone(), self.files.clone());
        Rc::new(move |name, press, window, cx| {
            if let Some(kind) = files::Files::kind_of(&files, name, cx) {
                files::pressed(&agent, &files, &bench, (name, kind, press), window, cx)
            }
        })
    }

    pub(super) fn new(
        t: &Tokens,
        provider: &str,
        reader: Entity<Reading>,
        (bench, asks): (WeakEntity<Workbench>, UnboundedSender<Ask>),
        agent: String,
        files: Rc<RefCell<files::Files>>,
    ) -> Looks {
        let mut mono = gpui_kit::font(theme::MONO);
        mono.fallbacks = Some(gpui_kit::FontFallbacks::from_fonts(vec![
            theme::MONO_ICONS.into()
        ]));
        let text = Style {
            font: theme::UI.into(),
            mono,
            ink: t.text_primary,
            ink_dim: t.text_tertiary,
            ink_faint: t.text_disabled,
            accent: t.accent,
            line: Hsla { a: 0.8, ..t.line },
            line_soft: t.line_soft,
            selection: Hsla { a: 0.3, ..t.accent },
            mark_ground: Hsla { a: 0.1, ..t.accent },
            code_ink: t.accent,
            found: Hsla {
                a: 0.28,
                ..t.accent
            },
            found_current: t.accent,
            table_head: t.surface_hover,
            table_ground: Hsla {
                a: 0.72,
                ..t.line_soft
            },
            code_ground: Hsla {
                a: 0.72,
                ..t.surface_sunken
            },
            code: Some(code_colours(reader.clone())),
            fences: Some(schema::fences(bench.clone(), agent.clone(), asks.clone())),
            on_link: Some(Rc::new({
                let (bench, agent) = (bench.clone(), agent.clone());
                move |href: &str, window: &mut Window, cx: &mut App| {
                    link::open(href, &agent, &bench, window, cx)
                }
            })),
            file_of: Some(Rc::new({
                let files = files.clone();
                move |name: &str, cx: &mut App| files::Files::kind_of(&files, name, cx)
            })),
            file_icon: Some(Rc::new(
                |name: &str, kind, size, _: &mut Window, cx: &mut App| {
                    let (icon, glyph) = match kind {
                        gpui_transcript::FileKind::Folder => (None, crate::icons::Icon::Folder),
                        gpui_transcript::FileKind::File => {
                            (crate::file_icons::icon(name, cx), crate::icons::Icon::File)
                        }
                    };
                    Some(match icon {
                        Some(icon) => gpui_kit::img(icon).size(size).into_any_element(),
                        None => crate::ui::icon(glyph, f32::from(size))
                            .text_color(Tokens::get(cx).text_tertiary)
                            .into_any_element(),
                    })
                },
            )),
            link_icon: Some(Rc::new(|href: &str, size, cx: &mut App| {
                let mark = link::service_mark(href)?;
                Some(crate::ui::picture(
                    mark,
                    f32::from(size),
                    Tokens::get(cx).text_primary,
                ))
            })),
            on_file: Some(Rc::new({
                let (bench, agent, files) = (bench.clone(), agent.clone(), files.clone());
                move |name: &str, kind, press, window: &mut Window, cx: &mut App| {
                    files::pressed(&agent, &files, &bench, (name, kind, press), window, cx)
                }
            })),
            image: Some(Rc::new({
                let (bench, quiet) = (bench.clone(), t.text_tertiary);
                move |src: &str, _: &str, _: &mut Window, _: &mut App| {
                    content::image(src, &bench, &asks, quiet)
                }
            })),
            ..Style::default()
        };
        let thought = Style {
            text_size: px(12.5),
            ink: t.text_tertiary,
            italic: true,
            ..text.clone()
        };
        let user = Style {
            text_size: px(13.5),
            ..text.clone()
        };
        let designed = Styles {
            text: Rc::new(text),
            thought: Rc::new(thought),
            user: Rc::new(user),
        };
        Looks {
            styles: RefCell::new(designed.clone()),
            designed,
            scale: Cell::new(1.),
            line: Hsla { a: 0.8, ..t.line },
            bubble: Hsla {
                a: 0.72,
                ..t.line_soft
            },
            quiet: t.text_tertiary,
            faint: t.text_disabled,
            ink: t.text_primary,
            hover: t.surface_hover,
            rule: Hsla {
                a: 0.14,
                ..t.text_primary
            },
            accent: t.accent,
            provider: provider.to_owned(),
            reader,
            tints: Tints::new(t),
            bench,
            agent,
            pictures: RefCell::default(),
            files,
        }
    }
}
