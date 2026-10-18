# ADR 0015: One browser per account, seen from every device

- Status: Accepted
- Decision date: 2026-10-16

## Context

Agents use a browser to check what they build, record what they did and work in websites that
have no API. Today that browser is the desktop app's: each tab is a WebKit view inside its window
([ADR 0003](./0003-browser-tabs-are-webkit-child-webviews.md)), so a host with no window has no
browser, and a site the person signed in to on one host is signed out on every other.

Agents now run on hosts and the person joins from any device
([ADR 0014](./0014-agents-run-on-hosts-and-every-client-is-a-way-in.md)). The person wants every
device and every agent to share the same browser, with the same sign-ins, whichever host the
agent runs on.

Copying cookies between hosts does not get there. Many sites rotate a sign-in each time it is
used, so two copies log each other out, and a sign-in that appears from a new place is often
challenged or dropped.

## Decision

- **The browser belongs to the account, not to a host.** One browser per account holds one
  profile: cookies, page storage, history and open tabs. Exactly one copy of it runs at a time,
  so there is never anything to merge.
- **It runs as its own service, `sikemux-browser`, on its home.** The core stays free of WebKit
  ([ADR 0006](./0006-background-core.md)). On a Mac the service uses WebKit's own view with no
  window; on Linux it uses WPE WebKit. The core answers the agents' `browser_*` tools through it,
  with the same tool list and page scripts as today.
- **Its home is one host the person picks, and later a sandbox of its own.** An always-on host
  first; once sandboxes exist, a small sandbox per account that sleeps with its memory when
  nobody uses it and wakes when a device or agent does.
- **Every device sees the same tabs.** The phone app, the web app and desktop apps on other hosts
  receive a tab as a live picture and send back clicks, typing and scrolling, so the person can
  watch, drive or take over from any of them. A desktop app on the browser's own host draws its
  tabs natively, as today, so typing and scrolling there never get worse. Only tabs someone is
  looking at are streamed.
- **Every agent uses it, on any host.** An agent's `browser_*` call travels over the host link
  ([ADR 0013](./0013-hosts-link-to-run-agents-for-each-other.md)) to the browser's home. When an
  agent's page lives on its own host, such as a dev server on `localhost`, the link carries that
  port to the browser, which opens it as if it were local.
- **Agents see only the sign-ins they are allowed.** The person's own tabs use the whole profile.
  Each agent's tabs use a cookie store of their own inside the same browser, filled only with the
  sites the person allowed that agent or project, and what those sites change is written back to
  the profile. Nothing is allowed by default; email, password managers, banks and the Sikemux
  account are never allowed to agents. Every use is logged with the agent, the site and the time.
- **People sign in by taking over.** Passwords, one-time codes, captchas and new-place checks are
  done by the person in the browser's tab from any device, never typed by an agent. A page an
  agent needs a sign-in for asks the person through the inbox.
- **The profile moves; the browser does not run twice.** The profile is saved, encrypted under
  the account's vault key ([ADR 0016](./0016-the-account-vault.md)), whenever the browser goes
  idle or stops. A lease on the account says which host runs it. When its home goes away or the
  person moves it, another host takes the lease, restores the profile and carries on.

## Consequences

- A host with no window gets a browser, and a sign-in made once works for every agent the person
  allows, on every host.
- The browser always reaches the web from its home's address, so sites see one place. Moving it
  costs one new-place check, done by taking over from the phone.
- The home encodes video for every tab someone watches: in hardware on a Mac, in software on
  Linux. A far-away home adds the network's round trip to typing in a streamed tab.
- WPE and the Mac's WebKit share the engine but not every part around it, so media and network
  behaviour can differ between a Linux home and a Mac home.
- Pages an agent opens on another host's `localhost` depend on the host link being up.
- The browser's home is one place that can fail; the saved profile is what lets another host take
  over.

## Alternatives considered

- **A browser on every host, with sign-ins copied between them.** Rejected: sites that rotate a
  sign-in on use log the copies out, and sign-ins appearing from many places get challenged.
- **A browser per host, signed in separately on each, as Grok Bot does on its one computer per
  user.** Rejected: the person would sign in again on every host, and an agent would see a
  different browser depending on where it runs.
- **One shared cookie jar for every agent.** Rejected: any agent could act on any site the person
  is signed in to, and one compromised page could reach all of them.
- **Chromium for the service.** Rejected, as for the app: Sikemux stays on WebKit, so agents see
  what Safari users see.
