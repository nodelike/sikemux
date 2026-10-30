# gpui-transcript

A streaming chat view for [GPUI](https://gpui.rs). Rows keep stable ids, a
message's markdown grows by patches to its tree of blocks, and the list draws
only the rows in view, anchored to the bottom.

- `Markdown` holds one message's blocks. `write` applies a patch; the blocks
  it does not reach keep their laid-out text.
- `Pen::markdown(id, &doc, window, cx)` draws paragraphs, headings, lists,
  task boxes, code fences, tables, quotes, links, inline code, emphasis,
  details folds and footnotes. `Style`, shared as an `Rc`, sets the colours
  and fonts, and takes callbacks for links, fences and images. A message of
  many blocks remembers each block's size and lays out only the blocks that
  changed, placing only those in view, so a long answer costs the same each
  frame however long it grows. Folds the reader opens stay open while the
  message is held.
- `Transcript<R: Row>` is a view holding the rows in order, in a windowed,
  bottom-anchored list. Push it `Change`s: `Put` (replace by id or append),
  `Update`, `DropFrom`, `Keep`, `Prepend` and `Reset`. They are applied
  together when it next draws, at most once a frame. Rows added above never
  move what the reader sees. While the reader is at the end, the end stays
  in view. Near the top it calls `on_reach_top` for an earlier page. It
  emits `EmptyChanged` when it comes to hold rows or comes to hold none, for
  a host that draws something else in an empty transcript's place.
- The reader selects text across rows by dragging or shift-clicking, and ⌘C
  copies it: the `Copy` action, bound in the `Transcript` key context by
  `init`, which the app calls once. A copy reads the rows' own markdown,
  through `Row::docs`, so rows scrolled out of view copy too.
- `find(Some(Query))` marks every place a text is found in the rows' markdown, with
  case and whole words as the query says, and `step` moves between them and brings
  each into view. `found()` says where the reader is, and `FoundChanged` is emitted
  when that or the places change. The host draws its own find bar.

The crate draws no rows itself. You hand `Transcript::new` a function that
draws the row at an index with the `Pen` it is given, calling `markdown` for
the text the row holds, in the order `Row::docs` lists it. Outside a
transcript, `Pen::new(style)` draws markdown without selection.

## Features

- `fork`: uses what GPUI gains in [sikemux/gpui](https://github.com/sikemux/gpui):
  letter spacing for table headings, and inline code drawn at 0.88 of the text's
  size. Without it the crate builds on `gpui-pre` as published.

## The tree and its patches

Blocks arrive as JSON. Each block is an object tagged by `t` (`p`, `h`,
`quote`, `ul`, `ol`, `li`, `pre`, `hr`, `table`, `fndef`, `details`, and
`div`, whose `align` lines its blocks up to one side), and inline content uses
`em`, `strong`, `del`, `code`, `a`, `img`, `br`, `fnref` and `path`, with a
bare string for text. A paragraph of nothing but pictures, linked or not, draws
them side by side, as a README's badges sit. A `path` holds a name that
could be a file, as written; `Style::file_of` says whether it is one, and
`Style::on_file` opens it. A patch is `{ "n": units, "o": [op…] }`.
`n` is the text's length in UTF-16 units after the patch. Each op has a path
`p` of child indexes, through `c`, or through a table's `rows`, then cells,
then cell contents:

- `{ "p": […], "at": k, "c": [...] }`: the list at `p` keeps its first `k`
  entries and gains `c`.
- `{ "p": […], "at": k, "s": "..." }`: the text at `p`, or a code element's `v`,
  keeps its first `k` units and gains `s`.

A patch that does not end at the text's length leaves the tree stale until a
whole one arrives, one that starts with `{ "p": [], "at": 0, "c": … }`.
`is_current` says which case you are in. When the tree is stale, draw the text
as plain text.

## Example

```sh
FONTS=dir/with/ttf/files cargo run --release --example transcript -- --shots out/
```

It draws offscreen on a simulated clock at 120 Hz. It opens 2,000 rows and
scrolls them to the top with wheel events. Then it opens the last 200 and
scrolls up while earlier pages arrive, checking that each page leaves the view
where it was. Last, it streams a 4,000-token answer at 500 tokens a second. For
each phase it prints the CPU each frame's draw took, the CPU that costs at
120 Hz, and memory. `--shots` saves pictures of the window along the way.

## License

MIT OR Apache-2.0
