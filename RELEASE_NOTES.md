# Sikemux v0.6.1-nightly.3

Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## Before you update

- **Your phone needs 0.2.0-nightly.13 or later**, from TestFlight on iPhone or the Google Play testing track on Android.
- **This nightly links only with hosts on the 0.6.1 nightlies.** Hosts on stable 0.6.0 can't reach it.

## New since nightly.2

- **A goal's page reads as a summary.** It leads with Needs you, What happened, Decided, and Running and next. Sikemux writes what happened, and a click on a sentence opens the chat, commit or evidence behind it. A summary says how old its record is, and it knows where the goal's pull requests stand.
- **Goals tell you when they move.** A slice landing, a goal finishing or an ETA slipping sends one short notification on your Mac and phone, and a click opens the goal. Turn it off in Settings › Notifications.
- **Approve in the chat.** What an agent waits on you for now stands above its composer, as questions do: the exact command with Allow once and Refuse, or a gate's evidence beside its button. Answering there clears it from Home and the notification too.
- **Stalled owners lose their slices.** An owner whose chat closed loses its slice at once, and one that has left it untouched for 30 minutes loses it then. You're asked whether to resume it or hand it on. After Stop Everything, agents keep theirs.
- **ETAs are times on your clock.** A slice's wait reads "check · ETA 16:40" rather than a duration.
- **Chats open in Goals.** An agent's chat opens right there instead of throwing the window back to the workbench, and a chat you start from Home shows only what you wrote.
- **Closing a terminal's tab or pane ends its shell**, so rows no longer pile up in the rail.

## Fixes

- Editing a message keeps what Sikemux told the agent alongside it, such as the goal card it tracks.
- A reply ending "Yes or no?" shows the question it stands for, with Yes and No to tap.
- Agents get Bitbucket's and GitLab's tools whenever an account is signed in, not only in a folder with a remote on that host.
- Connector sign-in has its Rundeck username and password, Allow plain HTTP, and SigNoz email and password forms back.
- A board card whose owner closed counts as nobody's, so it can't stay stuck.
- Starting an agent from the board says why it can't.
- ⌘W in Goals never closes a tab you can't see.
- Anything you can click shows the pointer, and Home's rows stay readable in a narrow window.
- The account menu opens Connectors.

For the complete patch history, compare [`v0.6.1-nightly.2...v0.6.1-nightly.3`](https://github.com/nodelike/sikemux/compare/v0.6.1-nightly.2...v0.6.1-nightly.3).
