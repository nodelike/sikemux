"""The desk's checks in a real window, on a throwaway core with fake agents only.

python3 -I ci/desk-checks.py --app APP --core CORE --agent FAKE_AGENT --seed FAKE_CHAT --out DIR [--only DK3,DF1] [--behind]

APP is the app built with the `drive` feature, CORE the sikemux-core binary, FAKE_AGENT
sikemux-fake-acp-agent and FAKE_CHAT the fake_chat example, which seeds the chats. Every check
acts through the app's own driver and reads the answer from app state: the driver's `state`,
the core's layout.json and the app's log. Each prints PASS, FAIL or LOCAL (needs a person's
eyes, the clipboard, a device or a click on a spot only a screenshot finds) and leaves a
capture of the window in DIR. Exits 1 when any check fails. --behind opens the window without
taking the screen or the keys from whoever is at the machine, and leaves out the captures.
"""

import argparse
import http.server
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
BUNDLE_ID = "com.nodelike.sikemux.gpui.test"
TOKENS = ("GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN")

PAGE_ONE = """<!doctype html><title>Kit page one</title><h1>Kit page one</h1>
<p><a href="page2.html">Go to the second page</a></p>
<p><a href="file.zip" download>Download file.zip</a></p><input placeholder="type here">"""
PAGE_TWO = """<!doctype html><title>Kit page two</title><h1>Kit page two</h1>
<p><a href="page.html">Back to the first page</a></p>"""
SHOWN = "<!doctype html><title>Kit shown page</title><h1>Kit shown page</h1><p>From page_show.</p>"
TASKS = {"version": 1, "tasks": [{"id": "count", "label": "Count", "command": "i=0; while [ $i -lt 900 ]; do i=$((i+1)); echo line $i; sleep 0.2; done"}]}


class Run:
    def __init__(self, options):
        self.options = options
        self.out = Path(options.out).resolve()
        self.root = self.out / "run"
        self.results = []
        self.app = None
        self.processes = []
        self.preferences = {}
        self.sockets = None

    # The world the app runs in.

    def set_up(self):
        agent_guard()
        if self.root.exists():
            shutil.rmtree(self.root)
        for folder in ("home", "data", "project", "site", "bin"):
            (self.root / folder).mkdir(parents=True)
        project = self.root / "project"
        (project / "a.txt").write_text("".join(f"line {n}\n" for n in range(1, 11)))
        (project / "sikemux.json").write_text(json.dumps(TASKS))
        git = ["git", "-C", str(project), "-c", "user.name=test", "-c", "user.email=test@invalid"]
        subprocess.run([*git, "init", "-q"], check=True)
        subprocess.run([*git, "add", "-A"], check=True)
        subprocess.run([*git, "commit", "-qm", "seed"], check=True)
        site = self.root / "site"
        (site / "page.html").write_text(PAGE_ONE)
        (site / "page2.html").write_text(PAGE_TWO)
        (site / "shown.html").write_text(SHOWN)
        (site / "file.zip").write_bytes(os.urandom(2048))
        self.port = serve(site)
        self.site = f"http://127.0.0.1:{self.port}"

        # An unsigned core inside an app bundle trusts that bundle as its app, as a release's does; on
        # Linux it trusts the `sikemux` beside it. A dev core trusts examples/remote beside it, which seeds the chats.
        if sys.platform == "darwin":
            bundle = self.root / "Sikemux GPUI Test.app"
            (bundle / "Contents/MacOS").mkdir(parents=True)
            (bundle / "Contents/Info.plist").write_text(plist())
            self.app_exe = bundle / "Contents/MacOS/sikemux"
            helpers = bundle / "Contents/Helpers/Sikemux Core.app/Contents/MacOS"
        else:
            helpers = self.root / "app"
            self.app_exe = helpers / "sikemux"
        (helpers / "examples").mkdir(parents=True)
        shutil.copy(self.options.app, self.app_exe)
        self.core_exe = helpers / "sikemux-host"
        shutil.copy(self.options.core, self.core_exe)
        self.seed_exe = helpers / "examples/remote"
        shutil.copy(self.options.seed, self.seed_exe)
        self.agent_exe = self.root / "bin/sikemux-fake-acp-agent"
        shutil.copy(self.options.agent, self.agent_exe)

        version = json.loads(subprocess.run([self.core_exe, "--upgrade-info"], capture_output=True, text=True).stdout)["version"]
        (self.root / "data/preferences.json").write_text(json.dumps({
            "notificationsIntroduced": True, "keptRunningNoticeShown": True, "voiceDictation": False,
            "onboardingComplete": True, "lastSeenVersion": version, **self.preferences,
        }))
        home = self.root / "home"
        (home / "Downloads").mkdir()
        self.env = {key: value for key, value in os.environ.items() if key not in TOKENS}
        self.env.update(HOME=str(home), CFFIXED_USER_HOME=str(home), SIKEMUX_CONFIG_DIR=str(home / ".config/sikemux-gpui"),
                        SIKEMUX_APP_EXECUTABLE="/usr/bin/true", SIKEMUX_AGENT_SEARCH="path")
        # A socket's path must fit in 104 bytes, which a deep --out can pass.
        self.sockets = Path(tempfile.mkdtemp(prefix="desk-", dir="/tmp"))
        self.socket = self.sockets / "core.sock"
        self.drive_socket = self.sockets / "drive.sock"
        if sys.platform != "darwin":
            # A second start finds the running app by a socket in here, so it must not reach a real session's app.
            self.env["XDG_RUNTIME_DIR"] = str(self.sockets)
        core = subprocess.Popen([self.core_exe, "--socket", self.socket, "--data-dir", self.root / "data"],
                                env=self.env, stdout=open(self.root / "core.log", "w"), stderr=subprocess.STDOUT)
        self.processes.append(core)
        if os.environ.get("SANDBOX_WATCH"):
            self.processes.append(subprocess.Popen(["python3", "-I", os.environ["SANDBOX_WATCH"], str(core.pid), os.environ["SANDBOX_AGENTS"]]))
        until(lambda: self.socket.exists(), 15) or sys.exit("the core did not start; see run/core.log")
        self.agents = []
        for said in ("hello", "again"):
            seeded = subprocess.run([self.seed_exe, self.socket, self.agent_exe, said], env={**self.env, "FAKE_PROJECT": str(project)},
                                    capture_output=True, text=True, timeout=60)
            (self.root / "seed.log").open("a").write(seeded.stdout + seeded.stderr)
            self.agents += [line.split()[1] for line in seeded.stdout.splitlines() if line.startswith("chat ")]
        self.front = self.agents[-1] if self.agents else None
        self.launch()

    def launch(self):
        log = open(self.root / "app.log", "a")
        env = {**self.env, "SIKEMUX_GPUI_DRIVE": str(self.drive_socket), "SIKEMUX_CORE_SOCKET": str(self.socket)}
        if self.options.behind:
            env["SIKEMUX_GPUI_DRIVE_BEHIND"] = "1"
        self.app = subprocess.Popen([self.app_exe], env=env, stdout=log, stderr=subprocess.STDOUT)
        until(lambda: self.drive_socket.exists() and self.state(), 30) or sys.exit("the app's driver did not answer; see run/app.log")
        # The app must be the last client to say it is the window, or the seeding's browser calls go to a client that left.
        self.drive("register" if self.options.behind else "activate", "wait 1.5")
        if self.options.window:
            width, height = self.options.window.split("x")
            self.drive(f"resize {width} {height}", "wait 1")

    def tear_down(self):
        # The core deletes the chats' histories as it stops, and a failed check is read from them.
        history = self.root / "data/chat-history"
        if history.is_dir():
            shutil.copytree(history, self.out / "chat-history", dirs_exist_ok=True)
        for process in [self.app, *self.processes]:
            if process and process.poll() is None:
                process.terminate()
                try:
                    process.wait(5)
                except subprocess.TimeoutExpired:
                    process.kill()
        if self.sockets:
            shutil.rmtree(self.sockets, ignore_errors=True)

    # Talking to the app.

    def drive(self, *steps):
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
                sock.settimeout(90)
                sock.connect(str(self.drive_socket))
                sock.sendall(("\n".join(steps) + "\nend\n").encode())
                said = b""
                while not said.endswith(b"done\n"):
                    chunk = sock.recv(65536)
                    if not chunk:
                        break
                    said += chunk
        except OSError as error:
            return [f"driver: {error}"]
        return said.decode(errors="replace").splitlines()

    def state(self):
        for line in self.drive("state"):
            if line.startswith("did: state "):
                return json.loads(line[len("did: state "):])
        return None

    @property
    def agent(self):
        """The chat in front, which the composer's prompts go to, else the last one that was."""
        active = self.strip().get("active") or ""
        if active.startswith("agent:"):
            self.front = active.removeprefix("agent:")
        return self.front

    def desk(self, agent=None):
        return ((self.state() or {}).get("desks") or {}).get(agent or self.agent) or {}

    def layout(self):
        try:
            return json.loads((self.root / "data/layout.json").read_text())
        except (OSError, ValueError):
            return {}

    def strip(self):
        projects = self.layout().get("projects") or {}
        return next((strip for strip in projects.values() if strip.get("tabs")), {})

    def log(self):
        return (self.root / "app.log").read_text(errors="replace")

    def history(self):
        """Every chat's history, one event a line, oldest first, by chat."""
        found = {}
        for path in sorted((self.root / "data/chat-history").glob("*.jsonl")):
            lines = found.setdefault(path.name, [])
            for line in path.read_text(errors="replace").splitlines():
                try:
                    lines.append(json.loads(line))
                except ValueError:
                    continue
        return found

    def composer(self):
        width, height = (self.state() or {}).get("window", [1340, 881])
        return width * 0.32, height - 120

    def prompt(self, text, seconds=20):
        """Types `text` into the front chat's composer and sends it; answers what the tool said to that prompt."""
        def asked(line):
            event = line.get("event") or {}
            return event.get("kind") == "prompt" and (event.get("payload") or {}).get("text") == text

        before = {chat: sum(map(asked, lines)) for chat, lines in self.history().items()}
        x, y = self.composer()
        self.drive(f"click {x:.0f} {y:.0f}", "wait 0.3", f"text {text}", "wait 0.2", "key enter")

        def reply():
            for chat, lines in self.history().items():
                ours = [at for at, line in enumerate(lines) if asked(line)]
                if len(ours) <= before.get(chat, 0):
                    continue
                for line in lines[ours[before.get(chat, 0)] + 1:]:
                    for said in strings(line):
                        if said.startswith(("tool answered: ", "tool failed: ")):
                            return said
            return None

        return until(reply, seconds) or ""

    def go(self, url):
        """Types `url` into the desk's address and presses Enter, which hands the page the keys."""
        self.drive("key cmd+l", "wait 0.6", f"text {url}", "wait 0.3", "key enter")
        return bool(until(lambda: self.desk().get("pageFocused"), 8))

    def focus_page(self):
        return self.go(active_page(self.desk()).get("url", ""))

    def desks_per_agent(self):
        counts = {}
        def walk(pane):
            if pane.get("kind") == "desk":
                counts[pane.get("target")] = counts.get(pane.get("target"), 0) + 1
            for inner in pane.get("panes") or []:
                walk(inner)
        for strip in (self.layout().get("projects") or {}).values():
            for tab in strip.get("tabs") or []:
                walk(tab.get("pane") or {})
        return counts

    def capture(self, name):
        if self.options.behind:
            return
        if sys.platform != "darwin":
            window = subprocess.run(["xdotool", "search", "--onlyvisible", "--pid", str(self.app.pid)], capture_output=True, text=True).stdout.split()
            crop = []
            if window:
                info = subprocess.run(["xwininfo", "-id", window[-1]], capture_output=True, text=True).stdout
                box = dict(line.strip().split(":", 1) for line in info.splitlines() if ":" in line)
                number = lambda key: int(box[key].strip())
                crop = ["-crop", f"{number('Width')}x{number('Height')}+{number('Absolute upper-left X')}+{number('Absolute upper-left Y')}"]
            # The page is a child window in another visual, which a capture of the app's own window leaves black.
            subprocess.run(["import", "-window", "root", *crop, "+repage", self.out / f"{name}.png"])
            return
        number = subprocess.run(["swift", HERE / "window-number.swift", str(self.app.pid)], capture_output=True, text=True).stdout.strip()
        if number:
            subprocess.run(["screencapture", "-x", "-o", "-l", number, self.out / f"{name}.png"])

    # Reporting.

    def report(self, name, ok, detail):
        self.say(name, "PASS" if ok else "FAIL", detail)
        self.capture(name)

    def leave(self, name, why):
        self.say(name, "LOCAL", why)

    def say(self, name, word, detail):
        self.results.append((name, word, detail))
        print(f"{word} {name}: {detail}", flush=True)


def agent_guard():
    """Away from CI, whose runners have no agent CLIs, a run must start under the agent guard
    (sandbox_run in /tmp/gpui-proto/sandbox-env.sh): agent wrappers first on PATH, the core
    searching only that PATH, and a watcher for the core this run starts."""
    if os.environ.get("GITHUB_ACTIONS") == "true":
        return
    watch = os.environ.get("SANDBOX_WATCH", "")
    wrappers = str(Path(watch).parent / "wrap") if watch else None
    if not watch or os.environ.get("PATH", "").split(":")[0] != wrappers or os.environ.get("SIKEMUX_AGENT_SEARCH") != "path":
        sys.exit("refusing to start a core and app outside the agent guard; run it locally as "
                 "`. /tmp/gpui-proto/sandbox-env.sh && sandbox_run --unwatched python3 -I ci/<this script> ...`")


def until(check, seconds=8):
    end = time.time() + seconds
    while time.time() < end:
        found = check()
        if found:
            return found
        time.sleep(0.25)
    return None


FETCHED = {}


def serve(folder):
    class Pages(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *args):
            super().__init__(*args, directory=str(folder))

        def do_GET(self):
            FETCHED[self.path] = FETCHED.get(self.path, 0) + 1
            super().do_GET()

        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Pages)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server.server_address[1]


def plist():
    return f"""<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{BUNDLE_ID}</string>
<key>CFBundleExecutable</key><string>sikemux</string>
<key>CFBundleName</key><string>Sikemux GPUI Test</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>"""


def strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for item in value.values():
            yield from strings(item)
    elif isinstance(value, list):
        for item in value:
            yield from strings(item)


def pages(desk):
    return desk.get("pages") or []


def active_page(desk):
    return next((page for page in pages(desk) if page.get("active")), {})


# The checks, in the order they run: each leaves the window as the next expects.

def dp5(run):
    every = answered(run.prompt('tool app.console {}'))
    errors = answered(run.prompt('tool app.console {"errors":true}'))
    texts = [message.get("text", "") for message in errors.get("messages", [])]
    only = all(text.startswith("sikemux") or "panicked" in text for text in texts)
    run.report("DP5", bool(every.get("messages")) and only and errors.get("matched", 0) < errors.get("recorded", 0),
               f"all: {every.get('matched')} of {every.get('recorded')} lines; errors only: {errors.get('matched')} of {errors.get('recorded')}, all sikemux lines or panics {only}")


def dp3(run):
    said = run.prompt(json_tool("page.show", {"path": str(run.root / "site/shown.html"), "title": "Kit shown"}))
    on_desk = any(page.get("title") == "Kit shown page" for page in pages(run.desk()))
    run.report("DP3", said.startswith("tool answered") and not on_desk, f"answer {said[:160]!r}; on the desk too: {on_desk}")


def d5(run):
    run.prompt(json_tool("browser.navigate", {"url": f"{run.site}/page.html"}))
    first = until(lambda: (lambda page: page if page.get("url", "").endswith("/page.html") and not page.get("loading") else None)(active_page(run.desk())), 15) or {}
    fresh = run.desk().get("open") and not first.get("back") and not first.get("forward")
    focused = run.go(f"{run.site}/page2.html")
    second = until(lambda: (lambda page: page if page.get("url", "").endswith("/page2.html") and page.get("back") else None)(active_page(run.desk())), 15) or {}
    run.drive("key cmd+[", "wait 1.5")
    back = active_page(run.desk())
    run.drive("key cmd+]", "wait 1.5")
    forward = active_page(run.desk())
    run.report("D5", bool(fresh and second and back.get("url", "").endswith("/page.html") and back.get("forward") and forward.get("url", "").endswith("/page2.html")),
               f"first {first.get('url')} back/forward {first.get('back')}/{first.get('forward')}; then {second.get('url')}; page focused {focused}; ⌘[ → {back.get('url')}; ⌘] → {forward.get('url')}")


def dk3(run):
    run.focus_page()
    run.drive("key cmd+r", "wait 0.3")
    reloading = active_page(run.desk())
    url = reloading.get("url")
    x, y = run.composer()
    run.drive(f"click {x:.0f} {y:.0f}", "wait 0.3", "key cmd+[", "wait 1.5")
    still = active_page(run.desk()).get("url")
    run.report("DK3", url == still and url and url.endswith("/page2.html"),
               f"⌘R in the page kept {url} (loading {reloading.get('loading')}); ⌘[ in the chat left {still}")


def d6_dk4_dk5(run):
    before = len(pages(run.desk()))
    run.drive("key cmd+shift+t", "wait 1.5")
    opened = run.desk()
    run.report("D6", len(pages(opened)) == before + 1, f"pages {before} → {len(pages(opened))} after ⌘⇧T, active {active_page(opened).get('url')!r}")
    run.drive("key cmd+l", "wait 0.5")
    at = active_page(run.desk()).get("url")
    run.drive("key ctrl+tab", "wait 0.8")
    stepped = active_page(run.desk()).get("url")
    run.drive("key ctrl+shift+tab", "wait 0.8")
    back = active_page(run.desk()).get("url")
    tab = run.strip().get("active")
    x, y = run.composer()
    run.drive(f"click {x:.0f} {y:.0f}", "wait 0.3", "key ctrl+tab", "wait 1")
    strip_stepped = run.strip().get("active")
    run.drive("key ctrl+shift+tab", "wait 1")
    run.report("DK5", stepped != at and back == at and strip_stepped != tab,
               f"in the address ⌃Tab {at!r} → {stepped!r}, ⌃⇧Tab → {back!r}; in the chat ⌃Tab moved the strip {tab} → {strip_stepped}")
    run.drive("key cmd+l", "wait 0.5", "key cmd+w", "wait 1")
    closed = run.desk()
    run.report("DK4", closed.get("open") and len(pages(closed)) == before,
               f"⌘W in the address closed its page: pages {before + 1} → {len(pages(closed))}, desk open {closed.get('open')}")


def dk6(run):
    before = run.strip().get("active")
    focused = run.focus_page()
    run.drive("key cmd+shift+[", "wait 1")
    after = run.strip().get("active")
    run.drive("key cmd+shift+]", "wait 1")
    run.report("DK6", focused and after != before, f"page focused {focused}; ⌘⇧[ moved the strip {before} → {after}")


def d7(run):
    x, y = run.composer()
    if not run.desk().get("open"):
        run.drive(f"click {x:.0f} {y:.0f}", "wait 0.3", "key cmd+j", "wait 1.5")
    urls = [page.get("url") for page in pages(run.desk())]
    run.drive(f"click {x:.0f} {y:.0f}", "wait 0.3", "key cmd+j", "wait 1")
    closed = run.desk().get("open")
    run.drive("key cmd+j", "wait 1.5")
    again = run.desk()
    run.report("D7", closed is False and again.get("open") and [page.get("url") for page in pages(again)] == urls,
               f"⌘J closed it: {closed is False}; ⌘J again: open {again.get('open')}, pages kept {urls == [page.get('url') for page in pages(again)]}")


def dk8(run):
    x, y = run.composer()
    if run.desk().get("open"):
        run.drive(f"click {x:.0f} {y:.0f}", "wait 0.3", "key cmd+j", "wait 1")
    closed = not run.desk().get("open")
    run.prompt(json_tool("browser.navigate", {"url": f"{run.site}/page.html"}))
    opened = until(lambda: run.desk().get("open"), 10)
    run.report("DK8", closed and bool(opened), f"desk closed first {closed}; the agent's navigate brought it out {bool(opened)}")


def dk2(run):
    x, y = run.composer()
    if not run.desk().get("open"):
        run.drive(f"click {x:.0f} {y:.0f}", "wait 0.3", "key cmd+j", "wait 1.5")
    run.drive(f"click {x:.0f} {y:.0f}", "wait 0.3", "key cmd+j", "wait 1")
    closed = run.desk().get("open") is False
    held = len(pages(run.desk()))
    run.drive("key cmd+l")
    desk = until(lambda: (lambda desk: desk if desk.get("addressFocused") else None)(run.desk()), 10) or run.desk()
    page = active_page(desk)
    run.report("DK2", closed and desk.get("open") and desk.get("addressFocused") and (held or page.get("url") == "about:blank"),
               f"closed first {closed}; ⌘L: open {desk.get('open')}, address focused {desk.get('addressFocused')}, page focused {desk.get('pageFocused')}, "
               f"keys in {(run.state() or {}).get('contexts', [])[-2:]}, on {page.get('url')!r} with {held} pages held")


def dk9(run):
    """Enter in the address with nothing changed reloads the page and hands it the keys."""
    run.prompt(json_tool("browser.navigate", {"url": f"{run.site}/page.html"}))
    until(lambda: active_page(run.desk()).get("url", "").endswith("/page.html") and not active_page(run.desk()).get("loading"), 15)
    before = FETCHED.get("/page.html", 0)
    run.drive("key cmd+l", "wait 0.6", "key enter")
    focused = until(lambda: run.desk().get("pageFocused"), 8)
    reloaded = until(lambda: FETCHED.get("/page.html", 0) > before, 8)
    run.report("DK9", bool(focused) and bool(reloaded), f"⌘L then Enter: page focused {bool(focused)}, fetched again {bool(reloaded)}")


def spot(run, name):
    return ((run.state() or {}).get("spots") or {}).get(name)


def pills(run):
    spots = (run.state() or {}).get("spots") or {}
    return {name: frame for name, frame in spots.items() if name.startswith((f"desk-page-{run.agent}-", f"desk-item-{run.agent}-"))}


def in_view(run, name):
    """Scrolls the desk's tab strip with the wheel, as a person would, until the pill `name` shows
    whole, and returns where it is then. A pill scrolled out of view has no spot, and one partly
    hidden at an edge reports only what shows, so its hidden edge is lined up with that one."""
    strip = spot(run, f"desk-tabs-{run.agent}")
    pill = spot(run, name)
    if not strip:
        return pill
    x, y = middle(strip)
    left, right = strip[0] + 0.5, strip[0] + strip[2] - 0.5
    earlier = True
    for _ in range(16):
        if pill:
            cut_left, cut_right = pill[0] < left, pill[0] + pill[2] > right
            if cut_left == cut_right:
                return pill
            earlier = cut_left
            shift = strip[0] + strip[2] - pill[0] - pill[2] if cut_left else strip[0] - pill[0]
        else:
            shift = strip[2] if earlier else -strip[2]
        before = pills(run)
        run.drive(f"scroll {x:.0f} {y:.0f} 0 {shift:.1f} 1", "wait 0.3")
        after = pills(run)
        if after == before:
            if pill:
                return pill
            earlier = not earlier
        pill = after.get(name)
    return pill


def close_in_view(run, name):
    """Scrolls the desk's tab strip later until the × of the pill `name` shows whole, as on a desk
    narrower than its pill, and returns where the × is then."""
    strip = spot(run, f"desk-tabs-{run.agent}")
    close = spot(run, name)
    if not strip:
        return close
    x, y = middle(strip)
    for _ in range(8):
        if close and close[0] + close[2] < strip[0] + strip[2] - 0.5:
            return close
        before = (run.state() or {}).get("spots")
        run.drive(f"scroll {x:.0f} {y:.0f} 0 {-strip[2] / 2:.1f} 1", "wait 0.3")
        close = until(lambda: spot(run, name), 1)
        if not close and (run.state() or {}).get("spots") == before:
            break
    return close


def middle(frame):
    left, top, width, height = frame
    return left + width / 2, top + height / 2


def show_kind(run, kind):
    """Clicks one of the desk's kinds, pages, files, terminals or simulator, as a person would."""
    frame = spot(run, f"desk-kind-{run.agent}-desk-{kind}")
    if frame:
        x, y = middle(frame)
        run.drive(f"click {x:.0f} {y:.0f}", "wait 1")
    return bool(frame)


def page_in_front(run):
    """Puts a page in front of the desk, past any file or terminal shown over its pages: the
    desk's Browser kind lists only its pages, so it is chosen first, as a person would."""
    if not pages(run.desk()):
        run.prompt(json_tool("browser.navigate", {"url": f"{run.site}/page.html"}))
        until(lambda: active_page(run.desk()).get("title"), 10)
    if run.desk().get("front"):
        show_kind(run, "pages")
    if run.desk().get("front"):
        frame = in_view(run, f"desk-page-{run.agent}-{active_page(run.desk()).get('title', '')}")
        if frame:
            x, y = middle(frame)
            run.drive(f"click {x:.0f} {y:.0f}", "wait 1")
    return until(lambda: run.desk().get("pageBounds"), 5)


def d8(run):
    before = page_in_front(run)
    width, height = (run.state() or {}).get("window", [1340, 881])
    run.drive(f"resize {width - 160:.0f} {height - 80:.0f}", "wait 1.5")
    state = run.state() or {}
    small = state.get("window", [0, 0])
    after = until(lambda: (lambda frame: frame if frame and frame != before else None)(run.desk().get("pageBounds")), 5) or run.desk().get("pageBounds")
    inside = bool(after) and after[0] + after[2] <= small[0] + 1 and after[1] + after[3] <= small[1] + 1
    run.drive(f"resize {width:.0f} {height:.0f}", "wait 1")
    run.report("D8", bool(before) and inside and after != before,
               f"window {width:.0f}x{height:.0f} → {small[0]:.0f}x{small[1]:.0f}; page {before} → {after}, inside the window {inside}")


def df2(run):
    """A narrow desk can close its file: its kinds switch between the page and the file, each
    pill shows what it names, and the file's × closes it and brings the page back."""
    path = str(run.root / "project/a.txt")
    if not pages(run.desk()):
        run.prompt(json_tool("browser.navigate", {"url": f"{run.site}/page.html"}))
        until(lambda: active_page(run.desk()).get("title"), 10)
    run.prompt(json_tool("ui.open", {"kind": "file", "path": path}))
    until(lambda: (run.desk().get("front") or {}).get("file") == path, 10)
    title = active_page(run.desk()).get("title", "")
    show_kind(run, "pages")
    page = in_view(run, f"desk-page-{run.agent}-{title}")
    if not page:
        return run.report("DF2", False, f"the page pill {title!r} never came into view (desk {spot(run, f'pane-Desk-{run.agent}')})")
    x, y = middle(page)
    run.drive(f"click {x:.0f} {y:.0f}", "wait 1")
    on_page = run.desk().get("front") is None
    show_kind(run, "files")
    item = in_view(run, f"desk-item-{run.agent}-a.txt")
    if not item:
        return run.report("DF2", False, f"the file pill never came into view (desk {spot(run, f'pane-Desk-{run.agent}')})")
    x, y = middle(item)
    run.drive(f"click {x:.0f} {y:.0f}", "wait 1")
    on_file = (run.desk().get("front") or {}).get("file") == path
    close = close_in_view(run, f"desk-close-{run.agent}-a.txt")
    if not close:
        return run.report("DF2", False, f"the file pill's × never came into view (file pill {item}, desk {spot(run, f'pane-Desk-{run.agent}')})")
    x, y = middle(close)
    run.drive(f"move {x:.0f} {y:.0f}", "wait 0.3", f"click {x:.0f} {y:.0f}", "wait 1")
    desk = run.desk()
    back = desk.get("front") is None and all(entry.get("file") != path for entry in desk.get("items") or [])
    run.report("DF2", on_page and on_file and back, f"page pill showed the page {on_page}; file pill showed the file {on_file}; its × brought the page back {back} "
               f"(front {desk.get('front')}, items {desk.get('items')}; page pill {page}, file pill {item}, its × {close}, desk {spot(run, f'pane-Desk-{run.agent}')})")


def dk7_keep(run):
    """Keep in Downloads, on the notice DK7's download left, moves the file there still quarantined."""
    if run.options.behind:
        return run.leave("DK7 Keep", "keeping reveals the file in Finder, which would come up over the person's work")
    keep = until(lambda: spot(run, "keep-download"), 5)
    if not keep:
        return run.report("DK7 Keep", False, "no Keep in Downloads button drawn after the download")
    downloads = run.root / "home/Downloads/file.zip"
    x, y = middle(keep)
    run.drive(f"click {x:.0f} {y:.0f}", "wait 2")
    kept = until(lambda: downloads.exists(), 8)
    marked = bool(kept) and "com.apple.quarantine" in subprocess.run(["xattr", "-l", downloads], capture_output=True, text=True).stdout
    run.report("DK7 Keep", bool(kept) and marked, f"kept in Downloads {bool(kept)}, still quarantined {marked}")


def one_desk(run):
    """Every way a desk opens, all at once with it closed, still leaves one desk."""
    x, y = run.composer()
    run.drive(f"click {x:.0f} {y:.0f}", "wait 0.3", "key cmd+j", "wait 1")
    run.drive("key cmd+l", "key cmd+shift+t", "key cmd+j")
    run.prompt(json_tool("ui.open", {"kind": "file", "path": str(run.root / "project/a.txt")}))
    run.prompt(json_tool("browser.navigate", {"url": f"{run.site}/page2.html"}))
    time.sleep(2)
    counts = run.desks_per_agent()
    run.report("ONE-DESK", counts.get(run.agent) == 1 and all(count == 1 for count in counts.values()), f"desks per agent {counts}")


def dk7(run):
    run.prompt(json_tool("browser.navigate", {"url": f"{run.site}/page.html"}))
    until(lambda: active_page(run.desk()).get("url", "").endswith("/page.html") and not active_page(run.desk()).get("loading"), 15)
    said = run.prompt(json_tool("browser.click", {"text": "Download file.zip"}))
    finished = until(lambda: "download Finished" in run.log() or "download finished" in run.log(), 20)
    started = "download Started" in run.log() or "download started" in run.log()
    run.report("DK7", started and bool(finished), f"click answer {said[:120]!r}; started {started}, finished {bool(finished)} (Keep in Downloads is checked locally)")


def df1(run):
    path = str(run.root / "project/a.txt")
    said = run.prompt(json_tool("ui.open", {"kind": "file", "path": path, "line": 3}))
    desk = until(lambda: (lambda desk: desk if (desk.get("front") or {}).get("file") == path else None)(run.desk()), 10) or {}
    def cursor_line():
        return next((file.get("line") for file in (run.state() or {}).get("files", []) if file.get("path") == path), None)

    line = until(lambda: cursor_line() == 3 and 3, 10) or cursor_line()
    run.report("DF1", bool(desk.get("open")) and line == 3, f"answer {said[:120]!r}; front {desk.get('front')}, cursor line {line}")


def df3(run):
    path = str(run.root / "project/a.txt")
    desk = spot(run, f"pane-Desk-{run.agent}")
    if desk:
        x, y = middle(desk)
        run.drive(f"click {x:.0f} {y:.0f}", "wait 0.5")
    contexts = (run.state() or {}).get("contexts", [])
    tabs = len(run.strip().get("tabs", []))
    run.drive("key cmd+w", "wait 1")
    desk = run.desk()
    gone = all(item.get("file") != path for item in desk.get("items") or [])
    run.report("DF3", gone and desk.get("open") and len(run.strip().get("tabs", [])) == tabs,
               f"keys in {contexts[-2:]}; ⌘W took the file off {gone}, desk open {desk.get('open')}, tabs {tabs} → {len(run.strip().get('tabs', []))}")


def df4(run):
    command = "i=0; while [ $i -lt 900 ]; do i=$((i+1)); echo line $i; sleep 0.2; done"
    said = run.prompt(json_tool("process.start", {"command": command, "label": "Count", "idempotencyKey": "desk-checks-df4"}))
    desk = until(lambda: (lambda desk: desk if (desk.get("front") or {}).get("terminal") else None)(run.desk()), 15) or {}
    run.execution = answered(said).get("executionId")
    run.report("DF4", desk.get("open") and (desk.get("front") or {}).get("terminal") == "Count",
               f"answer {said[:160]!r}; front {desk.get('front')}")


def df5(run):
    run.drive("key cmd+q")
    try:
        run.app.wait(15)
    except subprocess.TimeoutExpired:
        run.app.terminate()
        run.app.wait(5)
    run.launch()
    desk = until(lambda: (lambda desk: desk if any(item.get("terminal") for item in desk.get("items") or []) else None)(run.desk()), 20) or {}
    behind = not (desk.get("front") or {}).get("terminal")
    said = run.prompt(json_tool("ui.open", {"kind": "terminal", "executionId": run.execution})) if getattr(run, "execution", None) else "no executionId from DF4"
    front = until(lambda: (run.desk().get("front") or {}).get("terminal"), 10)
    run.report("DF5", bool(desk) and behind and bool(front),
               f"after the restart the terminal is on the desk {bool(desk)}, behind {behind}; ui.open brought it in front {bool(front)} ({said[:100]!r})")


def dk1(run):
    run.drive("key cmd+t", "wait 2")
    width, height = (run.state() or {}).get("window", [1340, 881])
    # A new terminal tab takes the keys only once it is clicked, as a person does to type in it.
    run.drive(f"click {width * 0.55:.0f} {height * 0.5:.0f}", "wait 0.5")
    terminal = run.strip().get("active")
    run.drive("key cmd+shift+t", "wait 2")
    after = run.strip().get("active")
    desk = run.desk()
    run.report("DK1", after != terminal and desk.get("open") and active_page(desk).get("url") is not None,
               f"terminal tab {terminal}; ⌘⇧T brought {after} forward, desk open {desk.get('open')}, pages {len(pages(desk))}")


def rs1(run):
    # A two-finger swipe over the tab rail turns its page; the rail's frame stays where it was.
    before = run.state() or {}
    x, y, width, height = before.get("railBounds") or [0, 0, 0, 0]
    if not width:
        return run.report("RS1", False, f"the state gave no rail bounds: {before.get('railBounds')!r}")
    centre = (x + width / 2, y + height / 2)
    turned = None
    for dx in (-300, 300):
        run.drive(f"scroll {centre[0]:.0f} {centre[1]:.0f} {dx} 0 20 momentum", "wait 1")
        after = run.state() or {}
        if after.get("rail") != before.get("rail"):
            turned = after
            break
    if turned is None:
        return run.report("RS1", False, f"neither swipe turned the rail's page from {before.get('rail')!r}")
    same = all(abs(a - b) < 0.5 for a, b in zip(before["railBounds"], turned.get("railBounds") or []))
    run.report("RS1", same, f"page {before.get('rail')} -> {turned.get('rail')}; frame {before['railBounds']} -> {turned.get('railBounds')}")


def tk1(run):
    """A clicked file tree row has the keys: Down moves them to the next row, F2 renames it."""
    run.drive("key cmd+e", "wait 1.5")
    rows = until(lambda: {name[len("tree-row-"):]: frame for name, frame in ((run.state() or {}).get("spots") or {}).items()
                          if name.startswith("tree-row-")} or None, 20) or {}
    ordered = sorted(rows.items(), key=lambda row: row[1][1])
    if len(ordered) < 2:
        state = run.state() or {}
        return run.report("TK1", False, f"rail {state.get('rail')} at {state.get('railBounds')}; tree {state.get('tree')}; "
                                        f"rows {list(rows)} of spots {sorted(state.get('spots') or {})}")
    (first, frame), (second, _) = ordered[0], ordered[1]
    x, y = middle(frame)
    run.drive(f"click {x:.0f} {y:.0f}", "wait 0.8", "key down", "wait 0.5")
    state = run.state() or {}
    keyed = (state.get("tree") or {}).get("keyed") or ""
    run.drive("key f2", "wait 0.8")
    renaming = ((run.state() or {}).get("tree") or {}).get("renaming") or ""
    run.capture("TK1")
    run.drive("key escape", "wait 0.5")
    run.report("TK1", keyed.endswith(f"/{second}") and renaming == keyed,
               f"clicked {first}, keys in {state.get('contexts', [])[-2:]}; Down keyed {keyed or None} (wanted {second}); "
               f"F2 renaming {renaming or None}")


def gk1(run):
    """A clicked Git change row has the keys: d opens its discard menu with its choices."""
    changed = run.root / "project/b.txt"
    changed.write_text("new\n")
    try:
        run.drive("key cmd+alt+3", "wait 2")
        row = until(lambda: spot(run, "git-file-b.txt"), 15)
        if not row:
            return run.report("GK1", False, f"no row for b.txt in the Git pane; git {(run.state() or {}).get('git')}")
        x, y = middle(row)
        run.drive(f"click {x:.0f} {y:.0f}", "wait 0.8", "key d", "wait 0.8")
        state = run.state() or {}
        git = state.get("git") or {}
        run.capture("GK1")
        run.drive("key escape", "wait 0.5")
        run.report("GK1", bool(git.get("menu")) and bool(git.get("choices")),
                   f"keys in {state.get('contexts', [])[-2:]}; menu {git.get('menu')!r} with {git.get('choices')}")
    finally:
        changed.unlink(missing_ok=True)


def local(run):
    for name, why in [
        ("DP1", "page_preview needs a terminal agent, which the fake agent is not"),
        ("DP2", "page_show from a terminal agent, as DP1"),
        ("DP4", "a theme switch goes through Settings' pickers"),
        ("D9", "no app popover reaches over the page body unless a menu is opened over it, at a spot only a screenshot finds"),
        ("S1", "needs a booted simulator and a person to judge the stream"),
        ("5.5", "⌘C and ⌘V use the clipboard"),
        ("5.6", "resizing needs a person's eyes"),
    ]:
        run.leave(name, why)


def json_tool(method, params):
    return f"tool {method} {json.dumps(params)}"


def answered(said):
    """The JSON a tool answered with, or an empty one when it failed or said none."""
    try:
        value, _ = json.JSONDecoder().raw_decode(said.removeprefix("tool answered: "))
        return value if isinstance(value, dict) else {}
    except ValueError:
        return {}


CHECKS = [("DP5", dp5), ("DP3", dp3), ("D5", d5), ("DK3", dk3), ("D6", d6_dk4_dk5), ("DK6", dk6), ("D7", d7), ("DK8", dk8),
          ("DK2", dk2), ("DK9", dk9), ("ONE-DESK", one_desk), ("DK7", dk7), ("DK7 Keep", dk7_keep), ("D8", d8), ("DF1", df1), ("DF3", df3), ("DF2", df2), ("DF4", df4), ("DF5", df5), ("DK1", dk1), ("RS1", rs1),
          ("TK1", tk1), ("GK1", gk1)]


def main():
    parser = argparse.ArgumentParser()
    for name in ("app", "core", "agent", "seed", "out"):
        parser.add_argument(f"--{name}", required=True)
    parser.add_argument("--only", default="")
    parser.add_argument("--behind", action="store_true")
    parser.add_argument("--window", help="WIDTHxHEIGHT in points, as a small screen clamps the window to")
    options = parser.parse_args()
    only = {name.strip() for name in options.only.split(",") if name.strip()}
    run = Run(options)
    signal.signal(signal.SIGTERM, lambda *_: (run.tear_down(), sys.exit(1)))
    try:
        run.set_up()
        print(f"agents {run.agents}; site {run.site}", flush=True)
        run.capture("start")
        for name, check in CHECKS:
            if only and name not in only:
                continue
            try:
                check(run)
            except Exception as error:
                run.report(name, False, f"the check broke: {error!r}")
            doubled = {agent: count for agent, count in run.desks_per_agent().items() if count > 1}
            if doubled:
                run.report(f"{name} one desk", False, f"agents with more than one desk after {name}: {doubled}")
        if not only:
            local(run)
    finally:
        run.tear_down()
    (run.out / "results.json").write_text(json.dumps(run.results, indent=1))
    sys.exit(1 if any(word == "FAIL" for _, word, _ in run.results) else 0)


if __name__ == "__main__":
    main()
