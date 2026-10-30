# sikemux design

Quiet, dense and beautiful, in the spirit of shadcn: neutral surfaces, one
hairline where two things meet, and colour only where it means something. Every
value lives in the app's `Tokens` (`app/src/theme.rs`); the share page
carries the same values as the CSS variables named below. Each component builds
its own look from those tokens.

## Principles

- **The chosen item is a grey lift.** A chosen row, tab, menu item or chip gets
  `--surface-chosen` across the whole item, with no colour and no edge. Hover is
  `--surface-hover`, one step lighter.
- **Purple is rare.** The accent is for keyboard focus, an on switch, a ticked
  checkbox or radio, a link, the HEAD badge, resize handles under the pointer
  and the one primary action on a surface. A choice from a set (a segmented
  button, a filter, a nav item, a choice card) is chosen, so it gets the grey
  lift; a chosen tab's underline is ink.
- **Hairlines, not shadows.** Flat surfaces separate with `--border`. Only things
  that float (menus, palettes, dialogs) get a shadow and may blur what is behind
  them.
- **Dense on purpose.** This is a tool people keep open all day. The type and
  spacing scales are small; let things fit rather than breathe. Sizes under 11px
  are deliberate, so do not round them up.
- **Tokens, not literals.** A hex or a one-off size in a component sheet should
  be rare and obviously bespoke. Every theme has to look right without it.

## Tokens

| Group    | Tokens                                                                                                                                            |
| -------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| Surfaces | `--surface-sunken`, `--surface`, `--surface-raised`, `--surface-overlay`                                                                          |
| States   | `--surface-hover` (6%), `--surface-chosen` (9%), `--surface-active` (10%), `--surface-selected` (accent, checked only)                            |
| Text     | `--text-primary`, `--text-secondary` (body), `--text-tertiary` (meta), `--text-disabled`                                                          |
| Borders  | `--border`, `--border-strong` (hover), `--border-selected` (a focused field)                                                                      |
| Ramp     | `--gray-100` … `--gray-1000`, mixed in oklab from the theme's ground and ink                                                                      |
| Semantic | `--danger`, `--live`, `--warn`, `--cmd`, the `--git-*` colours and the `--brand-*` agent colours                                                  |
| Radius   | `--radius-1` 4px chips · `--radius-2` 5px controls and rows · `--radius-3` 10px panels · `--radius-4` 14px floats                                 |
| Shadow   | `--shadow-1` … `--shadow-4`, from a pressed control to a modal                                                                                    |
| Focus    | `--focus-ring`, drawn outside the element so it never moves a layout                                                                              |
| Type     | `--ui` (Figtree) for what the app says, `--mono` for what the machine says; `--text-3xs` 10px … `--text-2xl` 28px, 13px by default; three weights |
| Space    | `--space-0` 2 · `-1` 4 · `-1h` 6 · `-2` 8 · `-3` 12 · `-4` 16 · `-5` 20 · `-6` 24                                                                 |
| Motion   | `--dur-1` 90ms · `--dur-2` 140ms · `--dur-3` 220ms, `--ease-out` in, `--ease-in-out` between; all zero under reduced motion                       |

A theme is one entry in the core's `themes/curated.json`; the app wears the palette
the core resolves from it (`theme::apply`), and every token above derives from it.

## Things that are easy to break

- **Window transparency.** Below full opacity the shell's ground takes the theme's
  colour at that opacity and the rails clear (`see_through.rs`). A new opaque
  surface must clear with them, or it shows as a dark slab over the desktop.
- **Blur on large surfaces.** Blur on the rails or the stage costs frame time
  next to busy terminals. Keep it to small floating surfaces.
