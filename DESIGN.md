# sikemux design system

The chrome is quiet so the code is loud. Everything below exists to keep it
that way.

This document is the contract between `src/styles/tokens.css` and the twenty-odd
component sheets that consume it. Read it before adding a colour, a corner or a
font size — almost every value you need already exists, and the ones that do not
are usually a sign the component is doing something the system should be doing.

---

## 1. Principles

**The accent is a scarce resource** (§4). It marks what is selected or focused,
and nothing else. A hovered row, a raised card, a header, a border, an icon — all
neutral. When every state is tinted, selection has nothing left to be, which is
exactly the failure the old chrome had.

**Hairlines, not shadows.** On a dark ground a 1px border separates two surfaces
better than any shadow, and costs nothing to composite. Shadows are reserved for
things that genuinely leave the frame: menus, palettes, dialogs.

**Two families, one rule.** The sans (`--ui`) is for anything the _app_ says: a
button label, a row title, a heading. The mono (`--mono`) is for anything the
_machine_ says: a branch name, a timestamp, a shortcut, a path, a diff. If you
are unsure, ask who wrote the string.

**Density is a feature.** This is a tool people keep open all day. The type
scale is tight and the spacing scale is small on purpose. Do not "let it
breathe" — let it fit.

**Every theme gets everything.** No token is hard-coded to a palette. The whole
system derives from four values the theme bus writes on `:root`, so a new theme
is still one tuple in `src/themes/index.ts` and inherits the entire ramp.

---

## 2. Theming

The pipeline has three stages and no shortcuts around it:

```
src/themes/index.ts     a Theme object: chrome, editor, highlight, terminal
        │
        ▼  applyChrome() in src/themes/bus.ts
:root                   --bg --bg-dim --ink --ink-dim --acc --line …
        │
        ▼  src/styles/tokens.css
design tokens           --gray-100…1000, --surface-*, --border, --text-*, …
        │
        ▼
component sheets        never touch a theme variable directly
```

A `Theme` is four groups of keys (`CHROME_KEYS`, `EDITOR_KEYS`,
`HIGHLIGHT_KEYS`, `TERMINAL_KEYS`) declared as a tuple in `THEME_DATA`. The bus
writes the chrome group onto `:root` as custom properties, hands the editor and
highlight groups to CodeMirror as compartment extensions, and the terminal group
to every live xterm. `subscribeTheme` lets non-CSS consumers — the shader field
is the only one today — re-read colours in place instead of remounting.

**To add a theme:** append one tuple to `THEME_DATA`. That is the whole job. The
neutral ramp, borders, text levels, focus ring and backdrop all follow from it.

**In a component sheet, use the design tokens, not the theme variables.**
`--ink`, `--rail`, `--rail-2`, `--pane` and `--void` still exist and still work;
they are the raw input, and reaching for them directly is how three panels ended
up three slightly different darks. Prefer `--surface`, `--text-secondary`,
`--border`.

---

## 3. Colour

### The neutral ramp

Ten steps from the window's ground to full ink, mixed in **oklab** so the steps
are perceptually even. sRGB bunches the dark end — the first four steps came out
indistinguishable, which is precisely the range chrome lives in.

| Token         | Mix                    | Use                                      |
| ------------- | ---------------------- | ---------------------------------------- |
| `--gray-base` | the theme's `--bg-dim` | the window recess                        |
| `--gray-100`  | ink 4%                 | panel surfaces                           |
| `--gray-200`  | ink 7%                 | raised off a panel                       |
| `--gray-300`  | ink 11%                | **the default border**, overlay surfaces |
| `--gray-400`  | ink 15%                | dividers inside a dense list             |
| `--gray-500`  | ink 22%                | hover borders, strong edges              |
| `--gray-600`  | ink 32%                | disabled text                            |
| `--gray-700`  | ink 42%                | icons at rest                            |
| `--gray-800`  | ink 55%                | tertiary text, placeholders              |
| `--gray-900`  | ink 76%                | secondary text — **body default**        |
| `--gray-1000` | the theme's `--ink`    | primary text, selected rows              |

### Semantic aliases

Reach for these first; drop to a raw ramp step only when none of them says what
you mean.

| Surfaces            |                  | Text               |                   | Borders             |                    |
| ------------------- | ---------------- | ------------------ | ----------------- | ------------------- | ------------------ |
| `--surface-sunken`  | window recess    | `--text-primary`   | titles, selected  | `--border`          | default hairline   |
| `--surface`         | panels           | `--text-secondary` | body              | `--border-strong`   | hover, emphasis    |
| `--surface-raised`  | cards on a panel | `--text-tertiary`  | meta, placeholder | `--border-selected` | the selected thing |
| `--surface-overlay` | menus, palettes  | `--text-disabled`  | disabled          |                     |                    |
| `--on-accent`       | ink on a fill    | `--on-danger`      | ink on a danger   |                     |                    |

### State

| Token                | What it is                                         |
| -------------------- | -------------------------------------------------- |
| `--surface-hover`    | neutral ink tint, 6% — every hover in the app      |
| `--surface-active`   | neutral ink tint, 10% — pressed, expanded, open    |
| `--surface-selected` | accent, 14% — **the current thing, and only that** |
| `--border-selected`  | accent, 42% — the edge that goes with it           |

Body copy sits at `--text-secondary`, not full ink. A pane where every row is
`--gray-1000` glares, and again leaves selection nowhere to go.

### Colour that is not neutral

Four semantic colours survive from the theme and keep their meaning everywhere:
`--danger`, `--live` (success/running), `--warn`, `--cmd`. Git status colours
(`--git-added`, `--git-modified`, `--git-deleted`, `--git-untracked`,
`--git-renamed`) and the per-provider agent brand colours (`--brand-claude`,
`--brand-codex`, …) are deliberately theme-independent: a provider's identity
and a diff's meaning should not change because you switched to Ayu.

---

## 4. The accent budget

The single rule the chrome is built on, stated plainly because it is the one
that gets broken:

> **Hover is neutral. The accent means selected or focused. Nothing else.**

| State                     | Surface                                    | Text               |
| ------------------------- | ------------------------------------------ | ------------------ |
| rest                      | transparent                                | `--text-secondary` |
| hover                     | `--surface-hover`                          | `--text-primary`   |
| selected / open / current | `--surface-selected` + `--border-selected` | `--text-primary`   |
| focused (keyboard)        | `var(--focus-ring)`                        | unchanged          |

Hover and selection have to differ in **hue**, not just in strength. When they
were the same colour at two weights, resting the pointer on a row was
indistinguishable from having selected it — which is what the rails, the tabs
and the agent list all did before this rule existed.

**Allowed to be accent:** the selected row, the open menu button, the current
tab, the checked item, the focus ring, the HEAD ref badge, a link, a resize
handle under the pointer, and exactly one filled primary action per surface.

The resize handle is the one hover in the app that keeps the accent, because
there it is not decoration: a splitter has no other way to say it is draggable,
and the whole affordance is that it lights up under the pointer.

**Not allowed:** hover of any kind, container borders, card fills, category
tags, section headers, icon buttons at rest, hint text, or a backdrop. Every one
of those was accent at some point, and each one spent a little more of what
selection needed in order to stand out.

Filled accent surfaces take `--on-accent` (or `--on-danger`) for their ink —
never a literal. A near-black tuned for the default purple goes unreadable the
moment a theme picks a dark accent; those tokens are the window's own ground, so
they follow the theme instead of fighting it.

The four semantic colours (`--danger`, `--live`, `--warn`, `--cmd`), the git
status colours and the per-provider brand colours sit outside this budget. They
carry meaning, not state.

---

## 5. Rows and tabs

Every list row in the app is one rule, in `modern-shell.css`:

```css
.sess-row,
.proj-row,
.proj-child,
.agent-row {
    …;
}
```

Same height (28px, 26px for a child), same `--radius-2`, same three states from
§4, same `--dur-1` transition. Tabs (`.tab-wrap`) follow the identical grammar
at 26px — an unselected tab is a _label_, not an eighth box competing with the
one that matters.

It is a single rule because it used to be several that disagreed. The project
navigator was styled twice — once as a rounded row, then again further down as a
square monospace tree — and the second block won, so half the rules were dead
and its rows gave no hover feedback at all.

**Adding a row type?** Add its selector to the existing rule rather than writing
a new block. If it genuinely has to differ, it should differ in one property,
not in a second definition of what a row is.

The tree spine (`.proj-children`) is one neutral hairline. The indent is what
says "child of"; tinting the line said it twice.

---

## 6. Overlays

Scrim, card, action — in that order. The confirm dialog is the one modal surface
in the product, so it sets the pattern the palettes follow.

**The scrim dims without tinting.** Mix it from `--gray-base`, never a literal:
three hardcoded near-blacks used to pull every theme's chrome toward purple on
the way down. Roughly 44% for a light overlay, 58–68% for a modal.

**The card floats, so it earns height.** `--surface-overlay`, a `--border`
hairline, `--radius-4`, and `--shadow-3` for a palette or `--shadow-4` for a
modal. This is the one place `backdrop-filter` is allowed — a 460px card can
afford `blur(20px)`; the rails and the stage cannot (§13).

**One filled action.** A footer's primary button is the only filled thing on the
surface, which is what makes it read as the answer. Everything beside it is a
bordered neutral, and its ink is `--on-accent` / `--on-danger`.

---

## 7. Type

Two families. `--ui` is Figtree (variable, 300–900). `--mono` is JetBrains Mono
Nerd Font. `--kbd` is the system sans, used only for shortcut glyphs, because
`⌘⌥⇧` render better in SF than in either.

| Token        | Size | Use                                               |
| ------------ | ---- | ------------------------------------------------- |
| `--text-3xs` | 10px | section labels, uppercase with `--tracking-label` |
| `--text-2xs` | 11px | timestamps, shortcut hints, counts                |
| `--text-xs`  | 12px | secondary rows, meta                              |
| `--text-sm`  | 13px | **the default** — rows, buttons, inputs           |
| `--text-md`  | 14px | panel titles                                      |
| `--text-lg`  | 16px | dialog titles                                     |
| `--text-xl`  | 20px | empty-state headings                              |
| `--text-2xl` | 28px | onboarding only                                   |

Tracking tightens as size grows, because Figtree sets loose at display sizes:
`--tracking-label` (0.08em) for uppercase labels, `--tracking-normal` (-0.005em)
for body, `--tracking-tight` (-0.015em) from `--text-md` up, and
`--tracking-display` (-0.028em) for `--text-xl` and above.

Three weights only: `--weight-normal` (400), `--weight-medium` (500),
`--weight-semibold` (600). Anything heavier reads as shouting at 13px.

---

## 8. Space

A 4px scale with two half steps, because 4px is already too big for an icon gap
in chrome this dense.

`--space-0` 2 · `--space-1` 4 · `--space-1h` 6 · `--space-2` 8 · `--space-3` 12
· `--space-4` 16 · `--space-5` 20 · `--space-6` 24

Gutters are the inset a surface keeps from its own edge: `--gutter-rail` (10px),
`--gutter-pane` (12px), `--gutter-row` (12px). The shell gutter — the recess
between the rails and the stage, and around the whole frame — is `--space-3`.

A literal in a component sheet means "this one is genuinely bespoke". If you are
writing `11px` because 8 and 12 both look wrong, the problem is usually the
element's height, not the scale.

---

## 9. Geometry

Four radii, chosen by what an element **is**, never by how big it is.

| Token           |       | Role                                                      |
| --------------- | ----- | --------------------------------------------------------- |
| `--radius-1`    | 4px   | badges, kbd marks, the smallest chips                     |
| `--radius-2`    | 6px   | **the default** — buttons, inputs, rows, tabs, menu items |
| `--radius-3`    | 10px  | rails, stage, panels, cards                               |
| `--radius-4`    | 14px  | palettes, dialogs, anything that floats                   |
| `--radius-full` | 999px | sliders and status dots that genuinely are capsules       |

Legacy aliases (`--radius-xs/sm/md/lg`, `--radius-chip/control/panel`) map onto
these so existing sheets get the geometry without every rule being rewritten.
New work should use the numbered scale.

The `---- Geometry ----` section of `modern-shell.css` enforces this app-wide
with `!important`, ordered general → specific. That is not laziness: nineteen
component sheets are imported ahead of it and most set their own corners, so
winning by specificity would mean a longer selector for every rule and a new one
each time a sheet grew a nested case. **The order is the mechanism** — the last
block mentioning a selector wins. Add to the block that matches the role.

---

## 10. Elevation

Shadows are two-part: a tight contact shadow that pins the element to what is
under it, and a wide ambient one that gives it height. One large blur reads as
fog.

| Token        | Use                           |
| ------------ | ----------------------------- |
| `--shadow-1` | pressed state, inline raise   |
| `--shadow-2` | dropdowns, tooltips, popovers |
| `--shadow-3` | palettes, peeked rails        |
| `--shadow-4` | modal dialogs                 |

**Flat surfaces get no shadow.** Rails, the stage, panes and cards separate with
`--border`. If you are reaching for a shadow on something that does not move,
you want a hairline.

---

## 11. Focus

One ring, everywhere:

```css
box-shadow: var(--focus-ring);
```

It is a solid inner edge in the window's own ground plus a soft accent halo,
drawn outside the element so it never changes a layout. The inner edge matters:
keyboard navigation lands on rows that are already accent-tinted by selection,
and a halo alone disappears against them.

Never remove a focus style without replacing it. `:focus-visible` is the right
selector for anything a mouse also clicks.

---

## 12. Motion

Chrome moves fast or not at all.

`--dur-1` 90ms (anything a keyboard can fire twice a second) · `--dur-2` 140ms
(hover, open/close) · `--dur-3` 220ms (panels arriving) · `--dur-4` 320ms (things
that cross the window).

`--ease-out` for anything entering, `--ease-in-out` for anything moving between
two places. All four durations collapse to `0ms` under
`prefers-reduced-motion: reduce`, so a transition written with the tokens is
already accessible — do not write a second reduced-motion rule for it.

---

## 13. Backdrop

The app sits on one continuous ground: a Paper Shaders field mounted on `.shell`
in `App.tsx`, plus an optional full-window image beneath it.

The shader is a Bayer dither over simplex noise, **monochrome** — it uses the
ramp's own grey, not the accent, because a coloured backdrop was the most
saturated thing on screen and broke the accent budget (§4). Rails, stage and the
gutters between them all sit on it, so the frame reads as one surface rather
than a texture that starts where the content does.

**Why you mostly see it in the gutter.** Panels keep `--panel-solidity` — six
points of glass — so the field ghosts through the whole app without a diff ever
competing with it. That is deliberately not `backdrop-filter`: the rails and the
stage are the largest elements on screen and every open terminal already runs a
WebGL renderer, so a real blur there is the one effect that would show in frame
times. Blur is spent on floating surfaces instead, where the area is small.

**The WebGL budget is the hard constraint.** A page gets roughly sixteen
contexts before the browser silently evicts the oldest, and every terminal takes
one for its renderer. `SURFACE_BUDGET` in `src/lib/shaderField.ts` is **2** — the
backdrop and the onboarding tour — and raising it trades a terminal's renderer
for decoration. Do not. Every mount is best-effort and silent; the interface
must look deliberate with no canvas in it, because there may not be one.

Tuning knobs, in order of how often you will want them:

| Property                   | Default | Effect                                                   |
| -------------------------- | ------- | -------------------------------------------------------- |
| `--shader-field-gain`      | `1`     | global strength for every field; `0` turns them all off  |
| `--backdrop-image`         | `none`  | a full-window image, e.g. `url("/backdrops/dunes.avif")` |
| `--backdrop-image-opacity` | `0.5`   | how present that image is                                |
| `--backdrop-blur`          | `0px`   | blur it, for photographic sources                        |

An image is strictly additive — `background-image: none` paints nothing, so the
default build costs a layer and no pixels. Keep any image dim and low-contrast.
A backdrop that competes with a diff is a backdrop that gets switched off.

---

## 14. Window transparency

`applyWindowOpacity` writes `--window-opacity` and, below 1, adds
`html.is-transparent`. `--surface-solidity` fades the panels with it, and
`--panel-solidity` keeps them six points behind so the backdrop still reads.
Once `is-transparent` is on, `base.css` clears the background on **every**
surface at once — rails, stage, panes alike — so the desktop shows through the
whole frame. Every surface goes together; they should never disagree about how
solid the window is. A new opaque surface must be added to that list.

---

## 15. Checklist

Before you add a rule:

- [ ] Is there a token for this? There usually is.
- [ ] Is this accent doing selection or focus work? If not, make it neutral (§4).
- [ ] Is this a list row? Add the selector to the shared grammar, do not
      write a second definition of what a row is (§5).
- [ ] Is this shadow on something that moves? If not, make it a hairline.
- [ ] Sans or mono — who is speaking, the app or the machine?
- [ ] Does the radius match the element's _role_, and is it in the right
      geometry block?
- [ ] Does it still read at `--window-opacity: 0.6`, and with no shader at all?
- [ ] Does it survive a theme switch — did you use a token, not a hex?
