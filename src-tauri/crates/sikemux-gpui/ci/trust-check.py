"""Whether an unsigned core knows its own window, and only it, by the bundle each program runs from.

python3 -I ci/trust-check.py --app APP --core CORE --agent FAKE_AGENT --seed FAKE_CHAT --out DIR [--behind]

The arguments are desk-checks.py's, whose bundle is laid out as a release is. The window quits and
opens again on the same core, which it did not start, as a person's does after a restart, then
switches projects, which only the window may do. A copy of the seeding program outside the bundle
must be refused, and the bundle's sikemux command must not be named as the window. Exits 1 when
any case fails.
"""

import importlib.util
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
APP_ONLY = "only Sikemux's own window or notch can do that"


def load(name, file):
    spec = importlib.util.spec_from_file_location(name, HERE / file)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


desk = load("desk_checks", "desk-checks.py")


def code_path(pid):
    named = subprocess.run(["swift", HERE / "code-path.swift", str(pid)], capture_output=True, text=True).stdout.strip()
    return os.path.realpath(named) if named else None


def project_id(run, folder):
    try:
        projects = json.loads((run.root / "data/workspace.json").read_text()).get("projects") or []
    except (OSError, ValueError):
        return None
    return next((project["id"] for project in projects if Path(project["path"]).resolve() == folder.resolve()), None)


def main():
    options = desk.argparse.ArgumentParser()
    for name in ("app", "core", "agent", "seed", "out"):
        options.add_argument(f"--{name}", required=True)
    options.add_argument("--behind", action="store_true")
    options = options.parse_args()
    options.only = ""
    options.window = None
    out = Path(options.out).resolve()
    run = desk.Run(options)
    results = []
    try:
        run.set_up()
        other = run.root / "other"
        other.mkdir()
        seeded = subprocess.run([run.seed_exe, run.socket, run.agent_exe, "other"], env={**run.env, "FAKE_PROJECT": str(other)},
                                capture_output=True, text=True, timeout=60)
        (run.root / "seed.log").open("a").write(seeded.stdout + seeded.stderr)

        run.app.terminate()
        run.app.wait(10)
        run.launch()
        wanted = project_id(run, other)
        row = desk.until(lambda: desk.spot(run, "project-row-other"), 15)
        if row:
            run.drive(f"click {row[0] + row[2] / 2} {row[1] + row[3] / 2}", "wait 1")
        switched = desk.until(lambda: wanted and run.layout().get("activeProject") == wanted, 10)
        refused = [line for line in run.log().splitlines() if APP_ONLY in line]
        results.append(("window-switches-project", bool(switched) and not refused,
                        f"reopened window clicked project 'other' ({'found' if row else 'no row'}); active now "
                        f"{run.layout().get('activeProject')}, wanted {wanted}; {len(refused)} of its requests refused as not the window"))

        elsewhere = run.root / "bin/remote"
        shutil.copy(run.seed_exe, elsewhere)
        third = run.root / "third"
        third.mkdir()
        outside = subprocess.run([elsewhere, run.socket, run.agent_exe, "third"], env={**run.env, "FAKE_PROJECT": str(third)},
                                 capture_output=True, text=True, timeout=60)
        said = (outside.stdout + outside.stderr).strip()
        results.append(("program-elsewhere-refused", outside.returncode != 0 and APP_ONLY in said,
                        f"the seeding program copied outside the bundle exited {outside.returncode}: {said[-200:]}"))

        bundle = run.app_exe.parent.parent.parent
        link = bundle / "Contents/MacOS/sikemux-host"
        link.symlink_to("../Helpers/Sikemux Core.app/Contents/MacOS/sikemux-host")
        command = subprocess.Popen([link, "core", "--socket", run.sockets / "cli.sock", "--data-dir", run.root / "cli"],
                                   env=run.env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        run.processes.append(command)
        if os.environ.get("SANDBOX_WATCH"):
            run.processes.append(subprocess.Popen(["python3", "-I", os.environ["SANDBOX_WATCH"], str(command.pid), os.environ["SANDBOX_AGENTS"]]))
        time.sleep(2)
        window, cli = code_path(run.app.pid), code_path(command.pid)
        results.append(("cli-not-the-window", window == os.path.realpath(bundle) and cli is not None and cli != window,
                        f"macOS names the window {window} and the bundle's sikemux command {cli}"))
    finally:
        run.tear_down()
    for name, ok, detail in results:
        print(f"{'PASS' if ok else 'FAIL'} {name}: {detail}", flush=True)
    out.mkdir(parents=True, exist_ok=True)
    (out / "results.json").write_text(json.dumps(results, indent=1))
    sys.exit(0 if results and all(ok for _, ok, _ in results) else 1)


if __name__ == "__main__":
    main()
