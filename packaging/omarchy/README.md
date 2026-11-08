# Sikemux on Omarchy

Two upstream pull requests make Sikemux an app Omarchy installs and removes from its own menu. Nothing here has been submitted yet.

| Order | Repository | What it adds | Files here |
|---|---|---|---|
| 1 | [omacom/omarchy-pkgs](https://github.com/omacom/omarchy-pkgs) | `sikemux-bin` in Omarchy's package repository, `pkgs.omarchy.org` | `omarchy-pkgs/pkgbuilds/sikemux-bin/`, `omarchy-pkgs/0001-Add-sikemux-bin.patch` |
| 2 | [omacom/omarchy](https://github.com/omacom/omarchy) | Sikemux under _Install > AI_ and _Remove > AI_, its icon in the Omarchy font, a remove script | `omarchy/0001-Add-Sikemux-to-Install-AI.patch`, `omarchy/files/` |

The menu PR installs `sikemux-bin` with `omarchy-pkg-add`, so it can only merge once the package is published. The package is on the fast ring, so merging PR 1 publishes it to `edge`, `rc` and `stable` together.

## Before either PR

- **A stable release with the Arch tarball.** The recipe downloads `Sikemux_<version>_amd64.tar.gz` (`usr/` plus `LICENSE`, owned by root) from the GitHub release `v<version>`. `scripts/build-linux.sh` packs it for every release built after 3f6ff0fb2; v0.6.0 and v0.6.1-nightly.6 have none. The upstream watch reads stable releases only, so a nightly cannot stand in.
- **The app leaves updates to pacman.** A pacman-owned `/usr/lib/sikemux` must never be replaced by the app or the core's root updater. The app should tell the person to update with pacman or `omarchy update`, as it does for the `.deb` and `.rpm`.
- **x86_64 only.** There is no aarch64 desktop build, so the recipe lists `arch=('x86_64')` and the install row is hidden off x86 (`"when":"omarchy-hw-x86"`, as Edge and Minecraft do). Omarchy prefers a missing build to be fixed upstream over a hidden row. Once Sikemux ships an aarch64 tarball, add `aarch64` with its own `source_aarch64` and drop the `when`.

## PR 1: `sikemux-bin` in omarchy-pkgs

### Submit

```bash
gh repo fork omacom/omarchy-pkgs --clone && cd omarchy-pkgs
git switch -c add-sikemux-bin
cp -R ~/proj/pers/sikemux/packaging/omarchy/omarchy-pkgs/pkgbuilds/sikemux-bin pkgbuilds/
cd pkgbuilds/sikemux-bin
# Set pkgver to the first stable release with the tarball, then fill its checksum:
sed -i 's/^pkgver=.*/pkgver=0.6.1/' PKGBUILD
updpkgsums                         # replaces the all-zero sha256sums_x86_64
rm -f Sikemux_*_amd64.tar.gz       # updpkgsums leaves the download here
cd ../..
./tests/pinned-sources.sh && ./tests/package-metadata.sh && ./tests/pkgname-unique.sh
python helpers/upstream-watch.py check pkgbuilds/sikemux-bin   # expect "already current"
bin/build --package sikemux-bin    # optional, needs Docker or Podman
git add pkgbuilds/sikemux-bin && git commit -m "Add sikemux-bin, the Sikemux desktop app"
gh pr create -R omacom/omarchy-pkgs --base master
```

`0001-Add-sikemux-bin.patch` is the same recipe as a commit against `master` (c9340c0), for `git am` instead of the copy. It carries the placeholder checksum and `pkgver=0.6.1`, so the two edits above still apply. The all-zero checksum is deliberate: unlike `SKIP`, makepkg rejects it, and the update bot never replaces a `SKIP`.

### What the recipe does

- Extracts the release tarball into the package with `bsdtar --no-same-owner`, so every file is root-owned, as the repository's owner check requires.
- The core keeps each account's sign-ins in `/var/lib/sikemux/<uid>` and runs setgid to the group `sikemux`, and package files must stay `root:root`. `sikemux.sysusers` creates the group. `sikemux.tmpfiles` makes the store directory `0770 root:sikemux`, and its `z` line re-applies `2755 root:sikemux` to `/usr/lib/sikemux/sikemux-host`. pacman's tmpfiles hook runs that line after every install and upgrade, so no `.install` script is needed. Both files are copies of `packaging/arch/sikemux.sysusers` and `sikemux.tmpfiles`; change them there and here together.
- `.omarchy/package.json` watches `nodelike/sikemux` GitHub releases with `v(?P<version>[0-9]+\.[0-9]+\.[0-9]+)`. The pattern has to match the whole tag, so `v0.6.1-nightly.6` and `mobile-v…` tags never match, and the watch also skips prereleases. The update bot then opens a PR for each stable release with the new version and checksum.
- The license is `LicenseRef-FSL-1.1-MIT`, as in `packaging/arch/PKGBUILD`, because namcap rejects `FSL-1.1-MIT`. The text is installed to `/usr/share/licenses/sikemux-bin/LICENSE`.
- `release_ring: fast`, as with `t3code-bin` and other vendor binaries. CI's `bin/check-fast-ring` confirms that `rc` and `stable` provide every library the app links.

### PR description

> **Add sikemux-bin, the Sikemux desktop app**
>
> Adds `sikemux-bin` <version>, [Sikemux](https://sikemux.com): an IDE for you and your coding agents. It's a native GPUI app, plus a core (`sikemux-host`) that keeps terminals and agents running after the window closes. The recipe packages the upstream release tarball, `Sikemux_<version>_amd64.tar.gz` from github.com/nodelike/sikemux, which carries the same `/usr` tree as the project's `.deb`.
>
> - **Root-owned files.** The tarball is extracted in `package()` with `bsdtar --no-same-owner`.
> - **Setgid core.** The core keeps each account's sign-ins in `/var/lib/sikemux/<uid>` and runs setgid to the `sikemux` group, so the agents a person runs (who are never in that group) can't read them. The package itself ships `root:root`. A sysusers.d file creates the group. A tmpfiles.d file creates `/var/lib/sikemux` (`0770 root:sikemux`) and sets `2755 root:sikemux` on `/usr/lib/sikemux/sikemux-host` after every install and upgrade, so there is no `.install` script.
> - **Updates.** The upstream watch on GitHub releases matches stable `vX.Y.Z` tags only, not the project's `-nightly.N` prereleases. The app doesn't update a pacman install itself.
> - **x86_64 only for now.** Upstream ships no aarch64 desktop build yet; it will be added here when it does.
> - **License.** FSL-1.1-MIT, declared as `LicenseRef-FSL-1.1-MIT` because namcap doesn't accept the SPDX name. The text is in `/usr/share/licenses/sikemux-bin/`.
> - **Fast ring.** It's a vendor binary.
>
> Tested on Omarchy 4.5 x86_64: `makepkg` built it, every file in the package is root-owned (`helpers/package-owners.sh`), `desktop-file-validate` passes on the desktop entry, and `ldd` finds every library both binaries link. `tests/pinned-sources.sh`, `package-metadata.sh`, `pkgname-unique.sh`, `package-owners.sh` and `build-matrix.sh` pass, and `helpers/upstream-watch.py check` reports the package current. <Add: installed with `pacman -U`, the core is `2755 root:sikemux` after install and after upgrade, the window opens from the launcher.>

Fill in the version, and the install line once you have run it.

## PR 2: Sikemux in the Omarchy menu

### Submit

Wait until PR 1 is merged and the package is listed, for example with `curl -fsSL https://pkgs.omarchy.org/stable/x86_64/omarchy.db | bsdtar -tf - | grep '^sikemux-bin-'`. Then:

```bash
gh repo fork omacom/omarchy --clone && cd omarchy
git switch -c add-sikemux
git am ~/proj/pers/sikemux/packaging/omarchy/omarchy/0001-Add-Sikemux-to-Install-AI.patch
for t in remove-ai i18n menu menu-guards menu-guard-budget; do bash test/shell.d/$t-test.sh >/tmp/$t.log 2>&1 || echo "$t failed"; done
./test/cli >/tmp/cli.log 2>&1 || echo "cli failed"
gh pr create -R omacom/omarchy --base quattro
```

The patch is a commit against `quattro` (d9cc361). If `git am` stops on a conflict because neighbouring menu rows moved, the plain files are in `omarchy/files/`. The other changes are:

- `default/omarchy/omarchy-menu.jsonc`, after `install.ai.perplexity` and after `remove.ai.perplexity` (the icon is the private-use character U+E920):
  ```jsonc
  "install.ai.sikemux": {"icon":"","iconFont":"omarchy","label":"Sikemux","when":"omarchy-hw-x86","disabled":"omarchy-pkg-present sikemux-bin","action":"omarchy-install-and-launch Sikemux sikemux-bin com.nodelike.sikemux"},
  "remove.ai.sikemux": {"icon":"","iconFont":"omarchy","label":"Sikemux","when":"omarchy-pkg-present sikemux-bin","action":"omarchy-launch-floating-terminal-with-presentation omarchy-remove-ai-sikemux"},
  ```
- `default/i18n/en/menu.json`: `install.ai.sikemux` and `remove.ai.sikemux` with label `Sikemux`. `default/i18n/en/cli.json`: `sikemux.deleteData`.
- The glyph: `OMARCHY_PATH=$PWD python3 bin/omarchy-dev-font add sikemux files/glyph/sikemux.svg --label Sikemux`, then point its line in `default/fonts/omarchy/README.md` at `brand/mark/mark-white.svg`. `sikemux.svg` is that mark with its two triangles and band joined into one path, which the font tool requires.
- `manual/17-ai.md`: Sikemux in the list of graphical AI apps.
- `test/shell.d/remove-ai-test.sh`: three checks for the remove script.

### What the patch does, and what it leaves out

- **Install** uses `omarchy-install-and-launch Sikemux sikemux-bin com.nodelike.sikemux`, the one-liner Amp, Grok Bot and Perplexity use. The last argument is the desktop entry the package installs.
- **Remove** is `bin/omarchy-remove-ai-sikemux`. It disables and deletes the core's user service (`com.nodelike.sikemux.core.service`), stops a running core with `sikemux-host stop`, falling back to `pkill`, quits the app, and drops the package. Then it asks, defaulting to no, whether to delete the person's data: `~/.local/share/com.nodelike.sikemux`, `~/.config/com.nodelike.sikemux`, `~/.config/sikemux`, and, with sudo, only this account's `/var/lib/sikemux/<uid>`. Without a terminal it keeps everything, as Perplexity and Hermes do. It never touches the agents' own state (`~/.claude`, `~/.codex`, …).
- **No Hyprland window rule.** On Linux, Sikemux opens one window (`app/src/main.rs`; the notch is macOS-only), so nothing needs floating or sizing.
- **No theme template.** Sikemux reads `~/.local/state/omarchy/current/theme/colors.toml` itself, which a template would only duplicate. `omarchy-theme-set` replaces the `theme` directory with `rm -rf` and `mv`, then writes `current/theme.generation`, so the app has to watch `current/`, not the theme directory. That was passed to the teammate doing the theming.
- **No nightly in Omarchy.** `sikemux-nightly-bin` stays in Sikemux's own Arch packaging; the menu installs stable only.

### PR description

> **Add Sikemux to Install > AI**
>
> Adds **Sikemux** to Install > AI, with a matching Remove > AI entry.
>
> - **Install:** `omarchy-install-and-launch Sikemux sikemux-bin com.nodelike.sikemux`, the same one-liner Amp, Grok Bot and Perplexity use. It's greyed out once `sikemux-bin` is installed and hidden off x86, because Sikemux has no aarch64 desktop build yet (it will be added to the package, and the `when` dropped, when it does). The icon is the Sikemux mark, added to the Omarchy font as `U+E920` with `omarchy dev font add`, from the project's `brand/mark/mark-white.svg` with its three shapes joined into one path.
> - **Remove:** the new `omarchy-remove-ai-sikemux` stops the core first, because it outlives the window and a user service can restart it at login. It disables and deletes that service, stops the core, quits the app, and drops the package. Like the Perplexity remover, it then asks, defaulting to no, whether to delete the person's chats, settings and mods, plus the sign-ins the core keeps for this account in `/var/lib/sikemux/<uid>` (removed with sudo, and only that account's). It never touches the agents' own state.
> - English labels and the prompt string are added to `default/i18n/en/`; the translation workflow fills in the other languages. The manual's list of graphical AI apps gains Sikemux.
> - No window rule: Sikemux opens a single window. No theme template: Sikemux follows `current/theme/colors.toml` itself.
>
> **Depends on omacom/omarchy-pkgs#<PR 1>**, which adds `sikemux-bin` on the fast ring. Merge this after that one has published.
>
> ### Tested
> - `test/shell.d/{remove-ai,i18n,menu,menu-guards,menu-guard-budget}-test.sh` and `./test/cli` pass on Omarchy 4.5. `remove-ai-test.sh` gains three checks: the core and its service are stopped before the package is dropped, nothing is deleted when nobody was asked, and a yes deletes the data and only this account's sign-ins.
> - The font renders the new mark beside the existing ones (`magick … label:`).
> - <Add once the package is published: clicked Install > AI > Sikemux in the live menu, the app opened; Remove > AI > Sikemux removed it.>

## How this was checked

On nexus (Omarchy 4.5, x86_64), working in a scratch directory without sudo:

- **The recipe:** built with `makepkg` from a stand-in tarball made from the v0.6.0 `.deb` the way `scripts/build-linux.sh` packs it, deliberately owned by uid 1000. The package's files are all root-owned (`helpers/package-owners.sh`), `desktop-file-validate` passes, `ldd` finds every library of `sikemux` and `sikemux-host`, and `.PKGINFO` has the expected name, provides and license. Installing it, and so the tmpfiles setgid step, needs root and was not done. Tiese's `packaging/arch/build-release.sh` checks install and upgrade in an Arch container.
- **omarchy-pkgs checks**, run against a clone with the recipe added: `tests/pinned-sources.sh`, `package-metadata.sh`, `pkgname-unique.sh`, `package-owners.sh` and `build-matrix.sh` pass. `helpers/upstream-watch.py check` reads v0.6.0 as the newest stable, ignoring the nightly and mobile tags. `bin/build --dry-run` plans the build.
- **Omarchy checks:** `remove-ai`, `i18n`, `menu`, `menu-guards` and `menu-guard-budget` tests and `./test/cli` pass with the patch. The `menu` test includes "every menu glyph drawn in the Omarchy font is one the font carries". The font row was rendered and looked at.
- **Both patches** apply with `git apply --check` to fresh checkouts of their base commits.

Not yet checked, because the package is not published: the menu rows in a live session, and a real install with the group and setgid in place.
