# ADR 0004: The terminal renders with WebGL

- Status: Accepted
- Decision date: 2026-09-21

## Context

xterm.js can draw with DOM spans or with its WebGL addon. With the DOM renderer,
any erase sequence refreshes the whole viewport, and prompts, `ls` and every
full-screen program send those constantly, so most output rebuilt every row as
new spans. After switching to WebGL, text looked slightly different, and going
back to the DOM renderer was proposed as the fix.

## Decision

WebGL is the terminal renderer (`src/terminal/renderer.ts`). The DOM renderer is
only a fallback when the WebGL context is lost. Problems with how text looks are
fixed in the WebGL output, not by changing renderer.

## Consequences

- The WebGL renderer rounds each cell down to a whole device pixel
  (`Math.floor(charWidth * dpr)`), where the DOM renderer kept the fraction.
  Complaints that text looks squeezed or different usually come from this
  rounding, not from the font.
- Its glyph atlas is not rebuilt when a web font finishes loading, because
  neither xterm nor the addon listens to `document.fonts`. The terminal has to
  trigger that itself.
