# Brand assets

The root `AGENTS.md` applies here too.

- Every logo and icon image lives in `brand/`. Take it from there; never draw or render
  the logo again.
  - `brand/mark/`: the bare S mark in white, black and purple (`#a277ff`), as SVG and
    transparent PNG at 64, 256 and 1024.
  - `brand/icon/`: the mark on the dark app gradient, or the purple "Sikemux Dev" one,
    with rounded or square corners, as SVG and PNG at 256 and 1024.
  - `brand/wordmark/`: the name lettered in Figtree as the site's nav sets it, alone or beside
    the mark, in white, black, or purple mark with white name, as SVG and PNG at 2048 wide.
  - `brand/macos/`: the real Dock icon (Liquid Glass) at 128 to 1024.
  - `brand/linux/`: the icon the Linux packages install, at 32 to 512.
  - `brand/social/avatar-400.png`: profile picture.
- The source is `packaging/icons/sikemux.icon`. After changing it, run `pnpm brand` to
  redraw everything; the Dock icon is taken from the installed `/Applications/Sikemux.app`
  (or `SIKEMUX_APP`), so install the new build first.
