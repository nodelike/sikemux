#!/usr/bin/env python3
"""Generate packaging/dmg/DS_Store deterministically without Finder or AppleScript.

Run with:
  uv run --with ds-store --with mac-alias python3 scripts/dmg-ds-store.py
"""

import datetime
from pathlib import Path

from ds_store import DSStore
from mac_alias import (
    ALIAS_HFS_VOLUME_SIGNATURE,
    ALIAS_KIND_FILE,
    Alias,
    TargetInfo,
    VolumeInfo,
)

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "packaging" / "dmg" / "DS_Store"

VOLUME_NAME = "Sikemux"
WINDOW_WIDTH = 660
WINDOW_HEIGHT = 400
TITLE_BAR_HEIGHT = 32
ICON_SIZE = 112.0
APP_POS = (168, 194)
APPLICATIONS_POS = (492, 194)


def make_background_alias() -> bytes:
    epoch = datetime.datetime(2026, 1, 1, 0, 0, 0, tzinfo=datetime.timezone.utc)
    volume = VolumeInfo(
        name=VOLUME_NAME,
        creation_date=epoch,
        fs_type=ALIAS_HFS_VOLUME_SIGNATURE,
        disk_type=0,
        attribute_flags=0,
        fs_id=b"\x00\x00",
    )
    target = TargetInfo(
        kind=ALIAS_KIND_FILE,
        filename="background.tiff",
        folder_cnid=0,
        cnid=0,
        creation_date=epoch,
        creator_code=b"\x00\x00\x00\x00",
        type_code=b"\x00\x00\x00\x00",
        levels_from=-1,
        levels_to=-1,
        folder_name=".background",
        carbon_path=f"{VOLUME_NAME}:.background:background.tiff",
        posix_path="/.background/background.tiff",
    )
    return Alias(volume=volume, target=target).to_bytes()


def main() -> None:
    if OUT.exists():
        OUT.unlink()

    with DSStore.open(str(OUT), "w+") as store:
        store["."]["vSrn"] = ("long", 1)
        store["."]["icvl"] = ("type", b"icnv")
        store["."]["bwsp"] = {
            "ContainerShowSidebar": False,
            "PreviewPaneVisibility": False,
            "ShowPathbar": False,
            "ShowSidebar": False,
            "ShowStatusBar": False,
            "ShowTabView": False,
            "ShowToolbar": False,
            "SidebarWidth": 0,
            "WindowBounds": f"{{{{200, 140}}, {{{WINDOW_WIDTH}, {WINDOW_HEIGHT + TITLE_BAR_HEIGHT}}}}}",
        }
        store["."]["icvp"] = {
            "arrangeBy": "none",
            "backgroundColorBlue": 0.039,
            "backgroundColorGreen": 0.031,
            "backgroundColorRed": 0.031,
            "backgroundImageAlias": make_background_alias(),
            "backgroundType": 2,
            "gridOffsetX": 0.0,
            "gridOffsetY": 0.0,
            "gridSpacing": 100.0,
            "iconSize": ICON_SIZE,
            "labelOnBottom": True,
            "scrollPositionX": 0.0,
            "scrollPositionY": 0.0,
            "showIconPreview": True,
            "showItemInfo": False,
            "textSize": 13.0,
            "viewOptionsVersion": 1,
        }
        store["Sikemux.app"]["Iloc"] = APP_POS
        store["Applications"]["Iloc"] = APPLICATIONS_POS

    print(f"DMG DS_Store written to {OUT} ({OUT.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
