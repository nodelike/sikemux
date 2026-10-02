# ADR 0003: Browser tabs are WebKit child webviews, not Chromium

- Status: Accepted
- Decision date: 2026-09-17

## Context

The in-app browser first ran a bundled Chromium (about 358 MB of runtime) and
showed each tab as a screencast driven over the Chrome DevTools Protocol, with
browser-use on top for agents. Nearly every browser bug people hit came from
that design: Enter not submitting forms, a blank pane after a resize, lost
focus, captchas failing. It was also slow to open and heavy to run.

## Decision

- Each browser tab is a Tauri child webview (WKWebView on macOS) placed over the
  React pane. The React side only reports where the tab should sit.
- Agents drive tabs through the harness methods (`browser.*`, declared in
  `browser/tools.json`), which act through a script injected into the page.
- Chromium, Electron and CEF are not options for the app or its browser. The
  app stays on Tauri and the system WebKit.

## Consequences

- WebKit quirks are worked around, never treated as a reason to switch engines.
  Known ones include hover needing an active page, a window blur that closes
  menus, and a 14,400 pt limit on PDF export.
- Pages that change their URL without a full load (`pushState`, hash changes)
  are not seen by the page-load hook.
- A child webview sits above the React layer, so anything drawn over a tab
  (menus, dialogs) has to hide the tab first, through `occludeNativeViews` in
  `src/state/nativeViews.ts`.
- Once a child webview exists, `get_webview_window("main")` returns nothing;
  use `get_window`.
