# Sikemux v0.6.1-nightly.4

Nightlies are signed and delivered exactly like stable releases, but they carry unreleased work and can break. Switch back to stable in Settings → About whenever you want; you keep the build you are on until a stable release passes it.

## Before you update

- **Your phone needs 0.2.0-nightly.13 or later**, from TestFlight on iPhone or the Google Play testing track on Android.
- **This nightly links only with hosts on the 0.6.1 nightlies.** Hosts on stable 0.6.0 can't reach it.

## New since nightly.3

- **Environments.** A project can name its environments, what recognises each (a Rundeck job, Vercel's target, an AWS profile, a kube context, a Terraform workspace or a command of the project's) and who passes it: nobody, the check, a person, or production. A deploy to a safe one goes, one after the check goes once it passes, and anything that reads as production or can't be recognised waits for you. A project with none set works as before.
- **Slices follow them.** A slice goes Built, then a step per environment. Each deploy is followed to its end and kept as evidence on the slice. Agents can only propose environments; you decide.
- **Held deploys say why.** Needs you and the agent's chat say where a held deploy is going and why it waited.

## Fixes

- An agent waiting on you shows as waiting everywhere: tabs, the tab rail, the project row and the notch. Only the question tool did before, so a question in a reply or a held deploy could show purple or nothing.
- A Chat from Home's composer no longer adds a goal card. Plan first and Hand off still do, and Hand off has its own icon.
- What happened on a goal reads clearly to someone who hasn't seen the code, and goals older than about 13 days keep their chats.
- `/` offers the agent's commands in a chat that waited for you.
- An agent's `git_commit` can push its own commits in a repository with attribution off.

For the complete patch history, compare [`v0.6.1-nightly.3...v0.6.1-nightly.4`](https://github.com/nodelike/sikemux/compare/v0.6.1-nightly.3...v0.6.1-nightly.4).
