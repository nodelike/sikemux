#!/usr/bin/env bash
# Names the frames of a macOS crash report (.ips) or a Sikemux panic report
# (autopsy/panic-*.json) with the release's dSYM, fetched from its GitHub release.
#   scripts/symbolicate.sh <report> [--dsym <path to .dSYM>]
set -euo pipefail

[[ $# -ge 1 ]] || { echo "Usage: $0 <crash.ips | panic-*.json> [--dsym <path>]" >&2; exit 2; }
REPORT="$1"
DSYM=""
[[ "${2:-}" == "--dsym" ]] && DSYM="${3:?--dsym needs a path}"

exec python3 -I - "$REPORT" "$DSYM" <<'PY'
import json, pathlib, shutil, subprocess, sys

report_path, dsym = pathlib.Path(sys.argv[1]), sys.argv[2]
text = report_path.read_text()
TEXT_BASE = 0x100000000

if report_path.suffix == ".ips":
    header, body = text.split("\n", 1)
    version = json.loads(header)["app_version"]
    body = json.loads(body)
    images = body["usedImages"]
    ours = {i for i, image in enumerate(images) if image.get("name") in ("sikemux", "sikemux-host")}
    if not ours:
        sys.exit("the report has no Sikemux image in it")
    image = images[min(ours)]
    part = "core" if image["name"] == "sikemux-host" else "app"
    uuid = image.get("uuid")
    thread = body["threads"][body["faultingThread"]]
    frames = [
        (index, frame["imageOffset"] if frame.get("imageIndex") in ours else None, frame.get("symbol"))
        for index, frame in enumerate(thread["frames"])
    ]
    print(f"{image['name']} {version}, thread {thread.get('name', body['faultingThread'])} crashed")
else:
    panic = json.loads(text)
    version = panic["version"]
    part = "app" if panic["process"] == "app" else "core"
    image = next((image for image in panic["images"]
                  if pathlib.Path(image.get("code_file", "")).name in ("sikemux", "sikemux-host")), None)
    if image is None:
        sys.exit("the report has no Sikemux image in it")
    load = int(image["image_addr"], 16)
    uuid = image.get("debug_id")
    frames = [(index, int(address, 16) - load, None) for index, address in enumerate(panic["frames"])]
    print(f"{panic['process']} {version} panicked on thread {panic['thread']}: {panic['message']}")
    print(f"at {panic['location']}")

def is_part(path):
    return path.name.startswith("sikemux-host") == (part == "core")

if not dsym:
    cache = pathlib.Path.home() / "Library/Caches/sikemux-symbols" / version
    name = f"Sikemux_{version}_aarch64.{part}.dSYM.zip"
    found = sorted(cache.glob("*.dSYM")) if cache.is_dir() else []
    found = [path for path in found if is_part(path)]
    if not found:
        cache.mkdir(parents=True, exist_ok=True)
        subprocess.run(["gh", "release", "download", f"v{version}", "-R", "nodelike/sikemux",
                        "-p", name, "-D", str(cache), "--clobber"], check=True)
        subprocess.run(["ditto", "-x", "-k", str(cache / name), str(cache)], check=True)
        found = [path for path in sorted(cache.glob("*.dSYM")) if is_part(path)]
    if not found:
        sys.exit(f"no dSYM in {name}")
    dsym = str(found[0])

dsym_uuids = subprocess.run(["dwarfdump", "--uuid", dsym], check=True, capture_output=True, text=True).stdout
if not uuid:
    print("warning: the report names no build UUID, so the dSYM cannot be checked against it", file=sys.stderr)
elif uuid.upper() not in dsym_uuids.upper():
    sys.exit(f"{dsym} is not the build that crashed: the report's UUID is {uuid}\n{dsym_uuids.strip()}")

offsets = [offset for _, offset, _ in frames if offset is not None]
names = subprocess.run(
    ["atos", "-arch", "arm64", "-o", dsym, "-l", hex(TEXT_BASE)] + [hex(TEXT_BASE + offset) for offset in offsets],
    check=True, capture_output=True, text=True,
).stdout
if shutil.which("rustfilt"):
    names = subprocess.run(["rustfilt"], input=names, check=True, capture_output=True, text=True).stdout
names = names.splitlines()
named = iter(names)
print()
for index, offset, symbol in frames:
    if offset is None:
        print(f"{index:>3}  {symbol or '(system)'}")
    else:
        print(f"{index:>3}  {next(named, '?')}")
PY
