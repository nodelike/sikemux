# Sikemux v0.5.4-nightly.4

The fourth nightly on the 0.5.4 line. Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## New since nightly.3

- **Hosts, one group.** Every host on your account links with the others on its own, and the app reaches each of them. In the rail, hosts are one group: pick a host the way you pick a project, and the tab rail lists what runs on it. New chats and terminals start on the chosen host, whose row says "here" and how busy it is. The choice survives a restart.
- **Adding hosts.** "Install Sikemux here" adds an SSH host to your account in one click, even when sudo asks for a password, and a headless host is set up by `sikemux login` alone (`sikemux pair` is gone). A device or host signed in to your account comes straight in, and every device hears when one joins, with Remove.
- **Hosts in Settings.** Each host is one row in Settings → Devices, renamed in place, and its name is the same everywhere. A host row says when it's connecting or out of reach, and the lock is reachable for every host.
- **Smoother under load.** Animations pause in panes and rail pages nobody can see, a chat nobody is looking at takes in its stream once a second, an agent changing state re-renders only its own project, and a hidden terminal flooding with output waits until it's shown. Buddies animate on the compositor.
- **Pictures in messages** sit where you put them and open from the composer.
- A chat opens on its last screen or two rather than its last 50 turns, so long chats open faster.
- A sleeping agent wakes as soon as its pane is shown, without a Sleeping card asking to resume it.
- Scrollbars show only while the pointer is over their list, floating over the rows.
- An image on a desk hides with its screen.

For the complete patch history, compare [`v0.5.4-nightly.3...v0.5.4-nightly.4`](https://github.com/nodelike/sikemux/compare/v0.5.4-nightly.3...v0.5.4-nightly.4).
