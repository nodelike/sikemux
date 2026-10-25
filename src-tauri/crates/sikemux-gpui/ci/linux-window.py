"""The GPUI app on Linux in a real X window, on a throwaway core with fake agents only.

python3 -I ci/linux-window.py --app APP --core CORE --agent FAKE_AGENT --seed FAKE_CHAT --out DIR [--browser [--input]]

The arguments are desk-checks.py's, run under an X server and a window manager (DISPLAY set). L1 opens the app
and checks it shows a window with the seeded chat. With --browser, L6 has the fake agent show a page in its
reply with page_show, which must draw in the chat, and L2 has it open a desk browser tab on a local page
through the agent browser tool, whose answer must report the desk's page area as its viewport; with --input as well, L4 has it type into the page's field,
press a key there and read the field back, L5 has the page ask a prompt the person sees in a dialog and the agent
answers, L9 records the tab for a few seconds and checks the video with ffprobe, and L3 clicks a link there, all
with the same tools. L7 starts the app again with a sikemux:// link to
the first chat, which must hand it to the running window and quit; L8, on a session bus with the desktop portal,
turns GNOME's animations off and on and needs the app's reduce-motion to follow. L2 and L4 also need the page drawn in their capture,
not a black or blank area. Each leaves a capture of the window in DIR. Exits 1 when any check fails.
"""

import argparse
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import time
import urllib.parse
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("desk_checks", HERE / "desk-checks.py")
desk = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desk)


def windows(pid):
    shown = subprocess.run(["xdotool", "search", "--onlyvisible", "--pid", str(pid)], capture_output=True, text=True)
    return shown.stdout.split()


def launch(run):
    time.sleep(3)
    state = run.state() or {}
    pane = (state.get("spots") or {}).get(f"pane-Agent-{run.agent}")
    shown = windows(run.app.pid)
    run.report("L1", bool(shown) and bool(pane), f"X windows of the app: {shown or 'none'}; chat pane {pane or 'not drawn'}; window {state.get('window')}")


def spread(shot, box):
    """The mean and standard deviation of a box's grey levels in a capture, from 0 to 1."""
    width, height, x, y = box
    said = subprocess.run(["convert", shot, "-crop", f"{width}x{height}+{x}+{y}", "+repage", "-colorspace", "Gray",
                           "-format", "%[fx:mean] %[fx:standard_deviation]", "info:"], capture_output=True, text=True).stdout.split()
    return tuple(map(float, said)) if len(said) == 2 else (0.0, 0.0)


def page_drawn(run, name, region=None):
    """Captures the window as `name` until a page area, the desk's unless `region` gives another, shows the
    page: varied over the whole area and in the band its heading sits in. Says whether it did, and what the
    area held."""
    seen = ["no page on the desk"]
    region = region or (lambda: run.desk().get("pageBounds"))

    def look():
        bounds = region()
        run.capture(name)
        shot = run.out / f"{name}.png"
        if not bounds or not shot.exists():
            return None
        pixels = subprocess.run(["identify", "-format", "%w", shot], capture_output=True, text=True).stdout
        scale = int(pixels) / (run.state() or {}).get("window", [pixels])[0]
        x, y, width, height = (round(value * scale) for value in bounds)
        mean, whole = spread(shot, (width, height, x, y))
        _, heading = spread(shot, (width, min(height, round(80 * scale)), x, y))
        seen[0] = f"page area {width}x{height}+{x}+{y}: mean {mean:.2f}, spread {whole:.3f}, heading band spread {heading:.3f}"
        return whole > 0.01 and heading > 0.05

    return bool(desk.until(look, 10)), seen[0]


# The height a chat gives an agent's page until the page reports its own (chat/page.rs).
DEFAULT_ROW = 320.0


def shown_in_chat(run):
    said = run.prompt("show " + json.dumps({"path": str(run.root / "site/shown.html"), "title": "Kit shown"}))
    row = lambda: next((frame for name, frame in ((run.state() or {}).get("spots") or {}).items()
                        if name.startswith("chat-page-")), None)
    # The row keeps the chat's default height until the page has loaded and reported its own.
    asked = time.time()
    loaded = desk.until(lambda: (row() or [0, 0, 0, DEFAULT_ROW])[3] != DEFAULT_ROW, 40)
    waited = f"the page reported its height {time.time() - asked:.1f} s after it was shown" if loaded else \
        "the page never reported its height"
    drawn, seen = page_drawn(run, "L6", row) if loaded else (False, "")
    run.say("L6", "PASS" if said.startswith("tool answered") and drawn else "FAIL",
            f"page_show answered {said[:120]!r}; {waited}; the chat's page row {row()}; {seen}")


def page_from_tool(run):
    said = run.prompt(desk.json_tool("browser.navigate", {"url": f"{run.site}/page.html"}))
    page = desk.until(lambda: (lambda page: page if page.get("url", "").endswith("/page.html") and not page.get("loading") else None)(desk.active_page(run.desk())), 20) or {}
    drawn, seen = page_drawn(run, "L2")
    reported = desk.answered(said).get("viewport") or {}
    frame = run.desk().get("pageBounds") or [0, 0, 0, 0]
    fits = [reported.get("width"), reported.get("height")] == [round(frame[2]), round(frame[3])]
    run.say("L2", "PASS" if page and drawn and fits else "FAIL",
            f"browser.navigate answered {said[:160]!r}; the desk's page {page.get('url')} titled {page.get('title')!r}; "
            f"it reported a {reported.get('width')}x{reported.get('height')} viewport for a "
            f"{round(frame[2])}x{round(frame[3])} page area; {seen}")


def typing_from_tool(run):
    typed = desk.answered(run.prompt(desk.json_tool("browser.type", {"selector": "input", "text": "hello linux"})))
    run.prompt(desk.json_tool("browser.press", {"key": "Backspace"}))
    read = desk.answered(run.prompt(desk.json_tool("browser.evaluate", {"script": "document.querySelector('input').value"})))
    drawn, seen = page_drawn(run, "L4")
    run.say("L4", "PASS" if typed.get("value") == "hello linux" and read.get("result") == "hello linu" and drawn else "FAIL",
            f"browser.type left {typed.get('value')!r}; after Backspace the field holds {read.get('result')!r}; {seen}")


def dialog_from_tool(run):
    before = windows(run.app.pid)
    run.prompt(desk.json_tool("browser.evaluate", {"script": "setTimeout(() => { window.answer = prompt('Your name?', 'nobody') }, 500), 'asked'"}))
    shown = desk.until(lambda: [window for window in windows(run.app.pid) if window not in before], 10) or []
    run.capture("L5-shown")
    said = run.prompt(desk.json_tool("browser.dialog", {"accept": True, "text": "linux"}))
    gone = desk.until(lambda: not [window for window in windows(run.app.pid) if window not in before], 10)
    read = desk.answered(run.prompt(desk.json_tool("browser.evaluate", {"script": "window.answer"})))
    run.report("L5", bool(shown) and bool(gone) and read.get("result") == "linux",
               f"the person's dialog {shown or 'never showed'}, {'gone' if gone else 'still open'} after browser.dialog answered "
               f"{said[:120]!r}; the page's prompt returned {read.get('result')!r}")


def recording_from_tool(run):
    started = desk.answered(run.prompt(desk.json_tool("browser.record", {"action": "start"})))
    time.sleep(3)
    stopped = desk.answered(run.prompt(desk.json_tool("browser.record", {"action": "stop"})))
    path = stopped.get("path") or ""
    probe = subprocess.run(["ffprobe", "-v", "error", "-count_frames", "-show_entries",
                            "stream=codec_name,nb_read_frames:format=duration", "-of", "json", path],
                           capture_output=True, text=True)
    found = json.loads(probe.stdout or "{}")
    stream = (found.get("streams") or [{}])[0]
    seconds = float((found.get("format") or {}).get("duration") or 0)
    frames = int(stream.get("nb_read_frames") or 0)
    run.report("L9", bool(started.get("recording")) and seconds >= 2 and frames >= 10,
               f"browser.record started {started.get('path')!r} and stopped at {path!r}: "
               f"{stream.get('codec_name')} for {seconds:.1f}s, {frames} frames")


def click_from_tool(run):
    said = run.prompt(desk.json_tool("browser.click", {"text": "Go to the second page"}))
    page = desk.until(lambda: (lambda page: page if page.get("url", "").endswith("/page2.html") else None)(desk.active_page(run.desk())), 20) or {}
    run.report("L3", bool(page), f"browser.click answered {said[:160]!r}; the desk's page after it {page.get('url')}")


def link_handoff(run):
    first = run.agents[0] if run.agents else None
    chats = (json.loads(path.read_text()) for path in (run.root / "data/chats").glob("*.json"))
    launch = next((chat["launch"] for chat in chats if (chat.get("launch") or {}).get("agentId") == first), None)
    if not launch:
        run.report("L7", False, f"no saved chat for the first seeded agent {first}")
        return
    link = f"sikemux://agent/{launch['provider']}/{launch['resumeId']}?project={urllib.parse.quote(launch['cwd'])}"
    before = run.strip().get("active")
    log = open(run.root / "second-start.log", "w")
    second = subprocess.Popen([run.app_exe, link], env={**run.env, "SIKEMUX_CORE_SOCKET": str(run.socket)}, stdout=log, stderr=subprocess.STDOUT)
    try:
        code = second.wait(10)
    except subprocess.TimeoutExpired:
        second.kill()
        code = None
    followed = desk.until(lambda: run.strip().get("active") == f"agent:{first}", 10)
    run.report("L7", code == 0 and before != f"agent:{first}" and bool(followed),
               f"the second start with {link} {'was still running after 10 s' if code is None else f'exited {code}'}; "
               f"the front tab went from {before!r} to {run.strip().get('active')!r}, wanted 'agent:{first}'")


def follows_motion(run):
    if not (shutil.which("gsettings") and os.environ.get("DBUS_SESSION_BUS_ADDRESS")):
        run.leave("L8", "needs a session bus with the desktop portal and gsettings")
        return
    motion = lambda: ((run.state() or {}).get("sky") or {}).get("reduceMotion")
    animations = lambda on: subprocess.run(["gsettings", "set", "org.gnome.desktop.interface", "enable-animations", on],
                                           capture_output=True, text=True).stderr.strip()
    before = motion()
    off = animations("false")
    still = desk.until(lambda: motion() is True, 10)
    on = animations("true")
    moving = desk.until(lambda: motion() is False, 10)
    run.report("L8", before is False and bool(still) and bool(moving),
               f"reduceMotion {before} at the start, {'true' if still else 'unchanged'} with animations off{f' ({off})' if off else ''}, "
               f"{'false again' if moving else 'unchanged'} with them on{f' ({on})' if on else ''}")


def main():
    parser = argparse.ArgumentParser()
    for name in ("app", "core", "agent", "seed", "out"):
        parser.add_argument(f"--{name}", required=True)
    parser.add_argument("--browser", action="store_true")
    parser.add_argument("--input", action="store_true")
    options = parser.parse_args()
    options.only, options.window, options.behind = "", None, False
    run = desk.Run(options)
    try:
        run.set_up()
        launch(run)
        if options.browser:
            shown_in_chat(run)
            page_from_tool(run)
        else:
            run.leave("L6", "waits for the Linux browser (pass --browser once it draws a page)")
            run.leave("L2", "waits for the Linux browser (pass --browser once it draws a page)")
        if options.browser and options.input:
            typing_from_tool(run)
            dialog_from_tool(run)
            recording_from_tool(run)
            click_from_tool(run)
        else:
            run.leave("L4", "waits for input in the Linux browser (pass --browser --input once keys reach a page)")
            run.leave("L5", "waits for input in the Linux browser (pass --browser --input once dialogs reach the agent)")
            run.leave("L9", "waits for input in the Linux browser (pass --browser --input to record a tab)")
            run.leave("L3", "waits for input in the Linux browser (pass --browser --input once clicks reach a page)")
        link_handoff(run)
        follows_motion(run)
    finally:
        run.tear_down()
    (run.out / "results.json").write_text(json.dumps(run.results, indent=1))
    sys.exit(1 if any(word == "FAIL" for _, word, _ in run.results) else 0)


if __name__ == "__main__":
    main()
