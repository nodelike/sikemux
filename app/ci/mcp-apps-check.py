"""MCP apps in a long chat, timed in a real window against a chat as long with none.

python3 -I ci/mcp-apps-check.py --app APP --core CORE --agent FAKE_AGENT --seed FAKE_CHAT --claude FAKE_CLAUDE --out DIR [--calls 31]

The arguments are desk-checks.py's, with FAKE_CLAUDE the sikemux-fake-claude binary. It seeds two
Claude chats: `tools N`, N calls to a tool with no app, and `apps N`, N calls whose tool names an
MCP App, each app calling its server once as it opens. In each it scrolls from the end to the
start and back, timing the gaps between frames and sampling the memory of the app and of the
WebKit content processes, and captures the window. Prints each run as a JSON line. Exits 1 when
the apps chat shows no app, its apps never reach their server, it runs more web content processes
than the app's limit, or its frames or memory fall behind the plain chat's past the margins below.
"""

import importlib.util
import json
import os
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
MOST_LIVE = 3
# How much worse than the plain chat the apps chat may be before the check fails.
SLOWEST_TWENTIETH_MS = 4.0
LONG_FRAMES = 3
MEMORY_MB = 400


def load(name, file):
    spec = importlib.util.spec_from_file_location(name, HERE / file)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


desk = load("desk_checks", "desk-checks.py")


def content_processes():
    """The WebKit content processes running, with their resident memory in MB."""
    listed = subprocess.run(["ps", "-axo", "rss=,command="], capture_output=True, text=True).stdout
    sizes = [int(line.split(None, 1)[0]) / 1024 for line in listed.splitlines() if "com.apple.WebKit.WebContent" in line]
    return len(sizes), sum(sizes)


def resident_mb(pid):
    said = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    return int(said) / 1024 if said else 0.0


class Sampler(threading.Thread):
    """Samples the app's and the content processes' memory four times a second."""

    def __init__(self, pid):
        super().__init__(daemon=True)
        self.pid, self.samples, self.going = pid, [], True

    def run(self):
        while self.going:
            count, web = content_processes()
            self.samples.append((count, web, resident_mb(self.pid)))
            time.sleep(0.25)

    def stop(self):
        self.going = False
        self.join()
        most = max(self.samples, default=(0, 0.0, 0.0), key=lambda sample: sample[1] + sample[2])
        return {
            "contentProcessesMost": max((sample[0] for sample in self.samples), default=0),
            "appMbMost": round(max((sample[2] for sample in self.samples), default=0.0), 1),
            "contentMbMost": round(max((sample[1] for sample in self.samples), default=0.0), 1),
            "totalMbMost": round(most[1] + most[2], 1),
        }


def seed(run, claude, prompt, calls):
    env = {**run.env, "FAKE_PROJECT": str(run.root / "project"), "FAKE_PROVIDER": "claude", "FAKE_PAUSE_MS": "500"}
    seeded = subprocess.run([run.seed_exe, run.socket, claude, prompt], env=env, capture_output=True, text=True, timeout=60)
    (run.root / "seed.log").open("a").write(seeded.stdout + seeded.stderr)
    chat = next((line.split()[1] for line in seeded.stdout.splitlines() if line.startswith("chat ")), None)
    said = f"Call {calls} of {calls}."
    desk.until(lambda: any(said in text for lines in run.history().values() for line in lines for text in desk.strings(line)), 60)
    return chat


def scroll_through(run, chat, label):
    """Scrolls the chat from its end to its start and back, a flick at a time with pauses as a
    person reading does, and answers the frame gaps and memory over the whole of it."""
    pane = desk.spot(run, f"pane-Agent-{chat}")
    if not pane:
        return None
    x, y = pane[0] + pane[2] / 2, pane[1] + pane[3] / 2
    sampler = Sampler(run.app.pid)
    sampler.start()
    run.drive("frames start")
    for direction in (1, -1):
        for flick in range(28):
            run.drive(f"scroll {x:.0f} {y:.0f} 0 {direction * 420} 14 momentum", "wait 0.25")
            if direction == 1 and flick == 6:
                run.capture(f"{label}-scrolling")
    frames = next((json.loads(line.split(" ", 2)[2]) for line in run.drive("frames") if line.startswith("did: frames ")), {})
    run.drive("wait 2.5")
    memory = sampler.stop()
    run.capture(f"{label}-end")
    return {"chat": label, **frames, **memory}


def main():
    options = desk.argparse.ArgumentParser()
    for name in ("app", "core", "agent", "seed", "out", "claude"):
        options.add_argument(f"--{name}", required=True)
    options.add_argument("--calls", type=int, default=31)
    options.add_argument("--behind", action="store_true")
    options = options.parse_args()
    options.only = ""
    options.window = None
    out = Path(options.out).resolve()
    calls_log = out / "app-calls.txt"
    os.environ["FAKE_CLAUDE_APP_CALLS"] = str(calls_log)
    run = desk.Run(options)
    failures = []
    try:
        run.set_up()
        claude = run.root / "bin/sikemux-fake-claude"
        claude.write_bytes(Path(options.claude).read_bytes())
        claude.chmod(0o755)

        plain_chat = seed(run, str(claude), f"tools {options.calls}", options.calls)
        run.drive("wait 3")
        run.capture("plain-start")
        plain = scroll_through(run, plain_chat, "plain")

        apps_chat = seed(run, str(claude), f"apps {options.calls}", options.calls)
        run.drive("wait 4")
        run.capture("apps-start")
        apps = scroll_through(run, apps_chat, "apps")
        print(json.dumps(plain), flush=True)
        print(json.dumps(apps), flush=True)
        (out / "mcp-apps.json").write_text(json.dumps({"plain": plain, "apps": apps}, indent=2))

        kept = list((run.root / "data/mcp-apps").glob("*.json"))
        reached = calls_log.read_text().splitlines() if calls_log.exists() else []
        if not plain or not apps:
            failures.append("a chat's pane was not found to scroll")
        if len(kept) < options.calls:
            failures.append(f"the core kept {len(kept)} apps of {options.calls}")
        if not reached:
            failures.append("no app reached its server")
        if apps and plain:
            if apps["contentProcessesMost"] > MOST_LIVE:
                failures.append(f"{apps['contentProcessesMost']} web content processes at once")
            if apps["p95"] > plain["p95"] + SLOWEST_TWENTIETH_MS:
                failures.append(f"slowest twentieth of frames {apps['p95']:.1f} ms against {plain['p95']:.1f} ms")
            if apps["over50"] > plain["over50"] + LONG_FRAMES:
                failures.append(f"{apps['over50']} frames past 50 ms against {plain['over50']}")
            if apps["totalMbMost"] > plain["totalMbMost"] + MEMORY_MB:
                failures.append(f"{apps['totalMbMost']} MB against {plain['totalMbMost']} MB")
        print(f"apps kept {len(kept)}, app calls that reached the server {len(reached)}", flush=True)
    finally:
        run.tear_down()
        for built in ("Sikemux GPUI Test.app", "app", "bin"):
            shutil.rmtree(run.root / built, ignore_errors=True)
    for failure in failures:
        print(f"FAIL mcp apps: {failure}", flush=True)
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
