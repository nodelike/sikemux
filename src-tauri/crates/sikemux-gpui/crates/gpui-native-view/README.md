# gpui-native-view

Shows a native view, such as a web page or a PDF, in a
[GPUI](https://github.com/sonorahq/gpui) element's place.

- `Surface` is what the element moves: anything that can be shown over a frame
  of the window and put out of sight again. `OwnedView` is one for a plain
  `NSView` of your own; a web view whose page size differs from its frame can
  implement `Surface` itself. `document::Document` is one for a file macOS
  draws itself: PDFs in PDFKit's view, office and iWork files in Quick Look's.
- `native_view(id, &surface)` is the element. It lays out like any other,
  hands the surface its frame whenever that changes, and puts the surface out
  of sight with the first frame that no longer draws it, even inside cached
  views.
- `.track_focus(&handle)` keeps GPUI's focus and the native keyboard together:
  clicking into the view focuses `handle`, focusing `handle` gives the view the
  keyboard, and focusing anything else in GPUI takes it back. Actions and key
  contexts on the element's ancestors work as for any focused element.

Native views sit above everything GPUI draws. Call
`Window::enable_scene_overlay` as the window opens: popovers, menus and other
deferred drawing then show above the views and take the clicks where they sit.
That needs GPUI with the window overlay plane, as in
[sikemux/gpui](https://github.com/sikemux/gpui).

```rust
let page = NativeSurface::new(OwnedView::new(pdf_view, window).unwrap());
native_view("page", &page).track_focus(&page_focus).size_full()
```

`cargo run --example text` shows a native text view beside a GPUI field and
under a popover, and names who has the keyboard.

Licensed under either of MIT or Apache-2.0, at your option.
