# sikemux design

Premium and dense, in the spirit of shadcn. Every value lives in the app's
`Tokens` (`app/src/theme.rs`); the share page carries the same values as the
CSS variables named below. This covers Sikemux's own UI, not pages or mockups
made for other apps.

## Settled

- A chosen row, tab or menu item gets `--surface-chosen` across the whole item;
  hover is `--surface-hover`. Never a coloured stripe down the left edge.
- Agent and provider marks keep their brand colour everywhere, whatever the
  agent's state.
- Group labels are spaced capitals with a hairline after them (PROJECTS, LIMITS).
- The project tree stays a tree: spine, tick, bare icons, and the current page
  in the accent, not filled pills.
- Picker rows stay one line.
- Chat transcript text is reading text at full ink; don't mute it.
- Sizes under 11px in the rails and lists are deliberate; don't round them up.
- Colours come from the tokens, so every theme looks right.
- UI icons are Lucide's, copied into `shared/ui/Icons.tsx` on its 24 grid at
  stroke 2 with round ends; a glyph Lucide lacks is drawn the same way.

## Tokens

| Group    | Tokens                                                                                                                                           |
| -------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| Surfaces | `--surface-sunken`, `--surface`, `--surface-raised`, `--surface-overlay`                                                                         |
| States   | `--surface-hover` (6%), `--surface-chosen` (9%), `--surface-active` (10%), `--surface-selected` (accent, checked only)                           |
| Text     | `--text-primary`, `--text-secondary` (body), `--text-tertiary` (meta), `--text-disabled`                                                         |
| Borders  | `--border`, `--border-strong` (hover), `--border-selected` (a focused field)                                                                     |
| Ramp     | Greys mixed in oklab from the theme's ground and ink, which the surfaces, borders and text above are taken from                                  |
| Semantic | `--danger`, `--live`, `--warn`, `--cmd`, the `--git-*` colours and the `--brand-*` agent colours                                                 |
| Radius   | `--radius-1` 4px chips · `--radius-2` 5px controls and rows · `--radius-3` 10px panels · `--radius-4` 14px floats                                |
| Shadow   | `--shadow-1` … `--shadow-4`, from a pressed control to a modal                                                                                   |
| Focus    | `--focus-ring`, drawn outside the element so it never moves a layout                                                                             |
| Type     | `--ui` (Figtree) for what the app says, `--mono` for what the machine says; `--text-3xs` 10px … `--text-xl` 20px, 13px by default; three weights |
| Space    | `--space-0` 2 · `-1` 4 · `-1h` 6 · `-2` 8 · `-2h` 10 · `-3` 12 · `-3h` 14 · `-4` 16 · `-5` 20 · `-6` 24                                          |
| Motion   | `--dur-1` 90ms · `--dur-2` 140ms · `--dur-3` 220ms, `--ease-out` in, `--ease-in-out` between; all zero under reduced motion                      |

A theme is one entry in the core's `themes/curated.json` or `themes/ghostty.json`,
or one the person made; the app wears the palette the core resolves from it
(`theme::apply`), and every token above derives from it.

## Things that are easy to break

- **Window transparency.** Below full opacity the shell's ground takes the theme's
  colour at that opacity and the rails clear (`see_through.rs`). A new opaque
  surface must clear with them, or it shows as a dark slab over the desktop.
- **Blur on large surfaces.** Blur on the rails or the stage costs frame time
  next to busy terminals. Keep it to small floating surfaces.
