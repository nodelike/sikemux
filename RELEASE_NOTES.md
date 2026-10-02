# Sikemux v0.4.3-nightly.6

The sixth nightly on the 0.4.3 line. Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## New since nightly.5

A quieter nightly: most of the work is groundwork for remote access from a phone, which is not released yet.

- **Paired devices see more of your Mac.** A paired device lists every chat you have open, including sleeping ones it can wake, by the titles the rail shows. It draws them in your Mac's theme, pane texture and picture, and is told which Sikemux release and channel your Mac runs, so it can say which side needs an update.
- **Steadier connections.** A device's chats are numbered, so a dropped connection picks up where it left off instead of starting over. A device can also unpair itself.
- **`sikemux core stop`** stops the background core from the command line.
- When Sikemux closes browser tabs in bulk, it now logs why.

For the complete patch history, compare [`v0.4.3-nightly.5...v0.4.3-nightly.6`](https://github.com/nodelike/sikemux/compare/v0.4.3-nightly.5...v0.4.3-nightly.6).
