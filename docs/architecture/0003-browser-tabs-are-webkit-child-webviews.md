# ADR 0003: Browser tabs are WebKit child webviews, not Chromium

- Status: Accepted
- Decision date: 2026-09-17
- Amended: 2026-10-16, by [ADR 0015](./0015-one-browser-per-account-seen-from-every-device.md): the
  browser belongs to the account and runs on one host; the desktop app there keeps drawing its
  tabs as WebKit views

## Context

The in-app browser first ran a bundled Chromium (about 358 MB of runtime) and
showed each tab as a screencast driven over the Chrome DevTools Protocol, with
browser-use on top for agents. Nearly every browser bug people hit came from
that design: Enter not submitting forms, a blank pane after a resize, lost
focus, captchas failing. It was also slow to open and heavy to run.

## Decision

- Each browser tab is a native WebKit view (WKWebView on macOS, WebKitGTK on
  Linux) that `sikemux-browser` adds to the app's window. The app only says where
  the tab should sit.
- Agents drive tabs through the harness methods (`browser.*`, declared in
  `browser/tools.json`), which act through a script injected into the page.
- Chromium, Electron and CEF are not options for the app or its browser. The
  browser stays on the system WebKit.

## Consequences

- WebKit quirks are worked around, never treated as a reason to switch engines.
  Known ones include hover needing an active page, a window blur that closes
  menus, and a 14,400 pt limit on PDF export.
- On Linux, pages that change their URL without a full load (`pushState`, hash
  changes) are not seen by the page-load hook. On macOS the tab watches the
  page's URL itself.
- Tabs sit beneath the window's overlay plane, which the app's GPUI fork adds, so
  menus and dialogs draw over a page without hiding it. A window that cannot
  make the plane shows pages over its menus.
