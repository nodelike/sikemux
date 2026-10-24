# gpui-terminal-grid

Draws a terminal's grid of cells in [GPUI](https://github.com/sonorahq/gpui).
It does not emulate a terminal. The cells come from whatever does: a local
emulator, or one in another process or on another machine that sends what
changed.

- `Grid` is what the drawing reads: rows of styled spans, a cursor and
  highlights such as a selection or search matches. Implement it over your
  own screen type, so nothing is copied.
- `Metrics` measures a font's cell, fits a size to columns and rows, and finds
  the cell under a point.
- `Layout::new` shapes each row in a `canvas` prepaint, and `Layout::paint`
  draws it. Box drawing, block and powerline characters are drawn as
  shapes that fill the cell, so borders join whatever the font.

```rust
canvas(
    move |bounds, window, cx| Layout::new(&my_grid(cx), bounds.origin, &metrics, window),
    |_, layout, window, cx| layout.paint(window, cx),
)
```

`cargo run --release --example grid -- out.png` draws a sample grid offscreen
and times a screen of fresh output.

Licensed under either of MIT or Apache-2.0, at your option.
