"""Whether the panes' grain and picture show in a real window, at every window opacity.

python3 -I ci/ground-check.py --app APP --core CORE --agent FAKE_AGENT --seed FAKE_CHAT --out DIR [--behind]

The arguments are desk-checks.py's. Each case starts the app on a throwaway core with its
pane preferences, captures the window and measures the middle of the chat pane: grain or a
picture spreads its pixels over many values where a pane covered by a flat fill has one.
At full opacity the pane must also be opaque, and below it see-through; behind a blur the
capture holds the blurred backdrop, so only the grain is checked there. Exits 1 when any case
fails.
"""

import importlib.util
import json
import struct
import subprocess
import sys
import time
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("desk_checks", HERE / "desk-checks.py")
desk = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desk)

CASES = [
    ("grain", {"paneShader": True, "paneStrength": 1.0, "windowOpacity": 1.0}, True),
    ("picture", {"paneShader": True, "paneStrength": 1.8, "windowOpacity": 1.0, "paneImage": "PICTURE"}, True),
    ("see-through", {"paneShader": True, "paneStrength": 1.0, "windowOpacity": 0.7, "windowBlur": 0}, False),
    ("blurred", {"paneShader": True, "paneStrength": 1.0, "windowOpacity": 0.7, "windowBlur": 20}, None),
]
# Where in the chat pane to look, from its top left: below its header, where the grain is densest.
BELOW_HEADER = 44
SPREAD = 2.0


def picture(path):
    """Writes a striped picture for the pane, so its dither has something to show."""
    width = height = 256
    rows = b"".join(b"\0" + bytes(((x // 16 + y // 16) % 2) * 200 + 30 for x in range(width) for _ in range(3)) for y in range(height))
    chunk = lambda kind, data: struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    path.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
                     + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


def pixels(path):
    """An 8-bit RGB or RGBA PNG's width, height, channels and rows."""
    data = path.read_bytes()
    at, idat, header = 8, b"", None
    while at < len(data):
        (length,), kind = struct.unpack(">I", data[at:at + 4]), data[at + 4:at + 8]
        body = data[at + 8:at + 8 + length]
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            idat += body
        at += 12 + length
    width, height, depth, colour = header[:4]
    if depth != 8 or colour not in (2, 6):
        raise ValueError(f"the capture is a PNG of depth {depth} colour {colour}")
    channels = 4 if colour == 6 else 3
    raw, stride = zlib.decompress(idat), width * channels
    rows, previous = [], bytearray(stride)
    for y in range(height):
        kind, line = raw[y * (stride + 1)], bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        for x in range(stride):
            left = line[x - channels] if x >= channels else 0
            up, corner = previous[x], previous[x - channels] if x >= channels else 0
            if kind == 1:
                line[x] = (line[x] + left) & 255
            elif kind == 2:
                line[x] = (line[x] + up) & 255
            elif kind == 3:
                line[x] = (line[x] + (left + up) // 2) & 255
            elif kind == 4:
                guess = left + up - corner
                near = min((abs(guess - left), left), (abs(guess - up), up), (abs(guess - corner), corner), key=lambda pair: pair[0])
                line[x] = (line[x] + near[1]) & 255
        rows.append(bytes(line))
        previous = line
    return width, height, channels, rows


def measure(path, frame, window):
    """The light's spread and the mean alpha in the top band of `frame`, a pane's left, top,
    width and height in the window's points, read from the capture at its own scale."""
    width, height, channels, rows = pixels(path)
    scale = width / window[0]
    pane_left, pane_top, pane_width, pane_height = frame
    left, right = int((pane_left + pane_width * 0.1) * scale), int((pane_left + pane_width * 0.5) * scale)
    top, bottom = int((pane_top + BELOW_HEADER) * scale), int((pane_top + pane_height * 0.2) * scale)
    lights, alphas = [], []
    for y in range(top, bottom):
        row = rows[y]
        for x in range(left, right):
            r, g, b = row[x * channels:x * channels + 3]
            lights.append(0.2126 * r + 0.7152 * g + 0.0722 * b)
            alphas.append(row[x * channels + 3] if channels == 4 else 255)
    mean = sum(lights) / len(lights)
    spread = (sum((light - mean) ** 2 for light in lights) / len(lights)) ** 0.5
    return spread, sum(alphas) / len(alphas)


def window_number(pid):
    """The app's largest window, on the screen or on another Space: not its menu bar strip,
    which on a small screen is as wide as the window."""
    script = f"""import CoreGraphics
let windows = CGWindowListCopyWindowInfo(.optionAll, kCGNullWindowID) as? [[String: Any]] ?? []
let mine = windows.filter {{ $0[kCGWindowOwnerPID as String] as? Int == {pid} && $0[kCGWindowLayer as String] as? Int == 0 }}
let area = {{ (window: [String: Any]) -> Double in
    let bounds = window[kCGWindowBounds as String] as? [String: Double] ?? [:]
    return (bounds["Width"] ?? 0) * (bounds["Height"] ?? 0)
}}
let biggest = mine.max {{ area($0) < area($1) }}
if let number = biggest?[kCGWindowNumber as String] as? Int {{ print(number) }}"""
    return subprocess.run(["swift", "-"], input=script, capture_output=True, text=True).stdout.strip()


def main():
    options = desk.argparse.ArgumentParser()
    for name in ("app", "core", "agent", "seed", "out"):
        options.add_argument(f"--{name}", required=True)
    options.add_argument("--behind", action="store_true")
    options = options.parse_args()
    out = Path(options.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    picture(out / "picture.png")
    results = []
    for name, preferences, opaque in CASES:
        options.out = str(out / name)
        options.only = ""
        options.window = None
        run = desk.Run(options)
        run.preferences = {key: str(out / "picture.png") if value == "PICTURE" else value for key, value in preferences.items()}
        try:
            run.set_up()
            time.sleep(4)
            state = run.state() or {}
            frame = (state.get("spots") or {}).get(f"pane-Agent-{run.agent}")
            if not frame:
                results.append((name, False, f"the app named no chat pane for {run.agent}"))
                continue
            shot = out / f"{name}.png"
            number = window_number(run.app.pid)
            if number:
                subprocess.run(["screencapture", "-x", "-o", "-l", number, shot])
            if not shot.exists():
                results.append((name, False, "no capture of the window"))
                continue
            spread, alpha = measure(shot, frame, state.get("window", [1340, 881]))
            shows = spread > SPREAD
            seen_through = alpha < 250
            ok = shows and (opaque is None or seen_through != opaque)
            wanted = {True: "opaque", False: "see-through", None: "any"}[opaque]
            results.append((name, ok, f"spread {spread:.1f} (over {SPREAD} shows grain or a picture), alpha {alpha:.0f} ({wanted} wanted)"))
        finally:
            run.tear_down()
    for name, ok, detail in results:
        print(f"{'PASS' if ok else 'FAIL'} {name}: {detail}", flush=True)
    (out / "results.json").write_text(json.dumps(results, indent=1))
    sys.exit(0 if all(ok for _, ok, _ in results) else 1)


if __name__ == "__main__":
    main()
