# Sikemux v0.6.1-nightly.12

Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## Before you update

- **Your phone needs 0.2.0-nightly.13 or later**, from TestFlight on iPhone or the Google Play testing track on Android.
- **This nightly links only with hosts on the 0.6.1 nightlies.** Hosts on stable 0.6.0 can't reach it.

## New since nightly.11

- **Android.** Agents drive Android emulators and phones with the same tools as iOS simulators, and the Devices desk shows them beside iOS simulators. Sikemux can set up the Android Emulator itself, without Android Studio, and Android modules offer their Gradle tasks with errors in the editor.
- **Google Play connector.** Agents see an app's tracks and keystores, and every upload waits for you.
- **Kotlin** files start JetBrains' kotlin-lsp.
- **A glance at a host** in the top bar's host picker: its state, CPU, memory and disk, and the apps using it most.
- **A new chat starts on the model and effort you last picked** for that agent, and agents can start teammates at a chosen effort.

## Fixes

- **Linux:** running out of memory stops only the agent or terminal that used it, not all of Sikemux.
- **AppImage:** its window can send prompts and sign in, and its browser opens https pages.
- **Updates on Linux:** the updater fetches both downloads before replacing either.
- **Drag and drop on Linux:** under XWayland on a scaled display, a dropped file lands where it's dropped.
- **Signing in:** a sign-in that can't finish says why. Signing in the Mac with Google, GitHub or Apple from a signed-out browser goes on to the consent card.
- **Agents:** a Codex that isn't signed in reads Signed out with Sign in, and a commit CI failed on passes once a later run that holds it passes.

For the complete patch history, compare [`v0.6.1-nightly.11...v0.6.1-nightly.12`](https://github.com/nodelike/sikemux/compare/v0.6.1-nightly.11...v0.6.1-nightly.12).
