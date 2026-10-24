#!/usr/bin/env python3
"""Writes the app's bundled mono font as TTF for GPUI, from the WOFF2 in public/fonts.
Needs fontTools and brotli. The icon glyphs share the Regular face's names there,
so they get a family of their own, which the text styles fall back to."""

import pathlib

from fontTools.ttLib import TTFont

crate = pathlib.Path(__file__).parent
source = crate / "../../../public/fonts"
out = crate / "assets/fonts"
ICONS_FAMILY = "JetBrainsMono NFM Icons"
ICONS_NAMES = {1: ICONS_FAMILY, 16: ICONS_FAMILY, 4: f"{ICONS_FAMILY} Regular", 6: "JetBrainsMonoNFMIcons-Regular"}

out.mkdir(parents=True, exist_ok=True)
for woff2 in sorted(source.glob("JetBrainsMonoNerdFontMono-*.woff2")):
    font = TTFont(woff2)
    font.flavor = None
    if woff2.stem.endswith("-Icons"):
        for record in font["name"].names:
            if record.nameID in ICONS_NAMES:
                record.string = ICONS_NAMES[record.nameID]
    font.save(out / f"{woff2.stem}.ttf")
