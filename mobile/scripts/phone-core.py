#!/usr/bin/env python3
"""Runs a phone-core build once per content key and shares its output between checkouts.

  phone-core.py --kind NAME --extra TEXT --out DIR [--stamp FILE] --keep PATH... -- COMMAND...

The key covers the Rust the phone links (sikemux-mobile and the local crates it reaches,
their Cargo.lock packages, the workspace's profiles and the toolchain), these scripts, and
--extra. A checkout whose files match the key is left alone; otherwise the store's copy is
cloned in, or COMMAND builds it and the result goes in the store. Checkouts asking for a key
another one is building wait for that build instead of starting their own.
  phone-core.py --key     prints the key
  phone-core.py --check   fails when the crates the key follows differ from cargo's own list
"""
import argparse
import contextlib
import fcntl
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time
import tomllib

REPO = os.path.realpath(os.path.join(os.path.dirname(__file__), "..", ".."))
STORE = os.path.expanduser("~/.cache/sikemux-phone-core")
ROOT_CRATE = "crates/sikemux-mobile"
KEPT_ENTRIES = 6
SCRIPTS = (
    "rust-toolchain.toml",
    "mobile/scripts/phone-core.py",
    "mobile/ios/scripts/build-core.sh",
    "mobile/native/scripts/ubrn.mjs",
    "mobile/native/ubrn.config.yaml",
)
BUILD_ENV = ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS")


def read_toml(path):
    with open(os.path.join(REPO, path), "rb") as file:
        return tomllib.load(file)


def dependency_tables(manifest):
    yield manifest.get("dependencies", {})
    yield manifest.get("build-dependencies", {})
    for target in manifest.get("target", {}).values():
        yield target.get("dependencies", {})
        yield target.get("build-dependencies", {})


def local_crates():
    """Each local crate the phone library links, by path, with the package names it depends on."""
    workspace = read_toml("Cargo.toml").get("workspace", {}).get("dependencies", {})
    crates, todo = {}, [ROOT_CRATE]
    while todo:
        crate = todo.pop()
        if crate in crates:
            continue
        manifest = read_toml(f"{crate}/Cargo.toml")
        names, base = set(), crate
        for table in dependency_tables(manifest):
            for name, spec in table.items():
                spec = spec if isinstance(spec, dict) else {}
                if spec.get("workspace"):
                    spec, base = (workspace.get(name) if isinstance(workspace.get(name), dict) else {}), "."
                else:
                    base = crate
                names.add(spec.get("package", name))
                if "path" in spec:
                    todo.append(os.path.normpath(os.path.join(base, spec["path"])))
        crates[crate] = (manifest["package"]["name"], names)
    return crates


def lock_closure(crates):
    packages = {}
    for package in read_toml("Cargo.lock")["package"]:
        packages.setdefault(package["name"], []).append(package)

    def pick(dependency):
        name, *version = dependency.split(" ")
        found = packages.get(name, [])
        return next((p for p in found if not version or p["version"] == version[0]), None)

    local = {name for name, _ in crates.values()}
    closure, queue = {}, []
    for name, wanted in crates.values():
        queue += [d for d in pick(name).get("dependencies", []) if d.split(" ")[0] in wanted]
    while queue:
        package = pick(queue.pop())
        if package is None or package["name"] in local:
            continue
        ident = (package["name"], package["version"])
        if ident not in closure:
            closure[ident] = package.get("checksum", package.get("source", ""))
            queue += package.get("dependencies", [])
    return closure


def content_key(extra):
    digest = hashlib.sha256()

    def add(label, data):
        digest.update(f"{label}\0{len(data)}\0".encode())
        digest.update(data)

    crates = local_crates()
    listed = subprocess.run(
        ["git", "-C", REPO, "ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", *sorted(crates)],
        capture_output=True,
        check=True,
    ).stdout.split(b"\0")
    for path in sorted(p.decode() for p in listed if p):
        full = os.path.join(REPO, path)
        if os.path.isfile(full):
            with open(full, "rb") as file:
                add(path, file.read())
    closure = lock_closure(crates)
    add("lock", "\n".join(f"{n} {v} {c}" for (n, v), c in sorted(closure.items())).encode())
    root = read_toml("Cargo.toml")
    shared = {"profile": root.get("profile"), "patch": root.get("patch"), "workspace": root.get("workspace", {}).get("dependencies")}
    add("manifest", json.dumps(shared, sort_keys=True).encode())
    for path in SCRIPTS + tuple(p for p in (".cargo/config.toml", ".cargo/config") if os.path.exists(os.path.join(REPO, p))):
        with open(os.path.join(REPO, path), "rb") as file:
            add(path, file.read())
    add("env", json.dumps({name: os.environ.get(name) for name in BUILD_ENV}).encode())
    add("extra", extra.encode())
    return digest.hexdigest()[:16]


def check():
    metadata = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", "aarch64-apple-ios"],
        cwd=REPO, capture_output=True, text=True, check=True,
    ).stdout
    metadata = json.loads(metadata)
    by_id = {p["id"]: p for p in metadata["packages"]}
    nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
    root = next(p["id"] for p in metadata["packages"] if p["name"] == "sikemux-mobile")
    linked, todo = set(), [root]
    while todo:
        node = nodes[todo.pop()]
        if node["id"] in linked:
            continue
        linked.add(node["id"])
        todo += [d["pkg"] for d in node["deps"] if any(k["kind"] in (None, "build") for k in d["dep_kinds"])]
    cargo_local = {
        os.path.relpath(os.path.dirname(by_id[i]["manifest_path"]), REPO) for i in linked if by_id[i]["source"] is None
    }
    followed = set(local_crates())
    if cargo_local != followed:
        print(f"phone-core.py follows {sorted(followed)}, but the phone links {sorted(cargo_local)}", file=sys.stderr)
        return 1
    print(f"phone-core.py follows every local crate the phone links: {sorted(followed)}")
    return 0


def clone(source, target):
    if os.path.isdir(target) and not os.path.islink(target):
        shutil.rmtree(target)
    elif os.path.lexists(target):
        os.unlink(target)
    os.makedirs(os.path.dirname(target), exist_ok=True)
    subprocess.run(["cp", "-c", "-R", "-p", source, target], check=True)


def evict():
    entries = [os.path.join(STORE, e) for e in os.listdir(STORE) if not e.endswith((".lock", ".new"))]
    entries.sort(key=os.path.getmtime, reverse=True)
    for entry in entries[KEPT_ENTRIES:]:
        shutil.rmtree(entry, ignore_errors=True)
        with contextlib.suppress(OSError):
            os.unlink(f"{entry}.lock")


def hold(lock_path, kind):
    lock = open(lock_path, "a+")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        lock.seek(0)
        builder = lock.read().strip() or "another checkout"
        print(f"phone-core: waiting for {builder}, which is building this same {kind} core.", file=sys.stderr)
        since = time.monotonic()
        fcntl.flock(lock, fcntl.LOCK_EX)
        print(f"phone-core: its build finished after {time.monotonic() - since:.0f}s.", file=sys.stderr)
    lock.seek(0)
    lock.truncate()
    lock.write(f"{REPO} (pid {os.getpid()})")
    lock.flush()
    return lock


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--key", action="store_true")
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--kind")
    parser.add_argument("--extra", default="")
    parser.add_argument("--out")
    parser.add_argument("--stamp", default="core.key")
    parser.add_argument("--keep", nargs="+", default=[])
    parser.add_argument("command", nargs="*")
    args = parser.parse_args()
    if args.check:
        return check()
    key = f"{content_key(args.extra)}-{args.kind}"
    if args.key:
        print(key)
        return 0
    if os.environ.get("CI"):
        return subprocess.run(args.command).returncode

    out = os.path.join(REPO, args.out)
    stamp = os.path.join(out, args.stamp)
    kept = [os.path.join(out, path) for path in args.keep]

    def current():
        try:
            with open(stamp) as file:
                return file.read().strip() == key and all(os.path.lexists(p) for p in kept)
        except OSError:
            return False

    if current():
        print(f"phone-core: {args.out} already holds core {key}.", file=sys.stderr)
        return 0
    os.makedirs(STORE, exist_ok=True)
    entry = os.path.join(STORE, key)
    lock = hold(f"{entry}.lock", args.kind)
    try:
        if os.path.isdir(entry):
            for path in args.keep:
                clone(os.path.join(entry, path), os.path.join(out, path))
            os.utime(entry)
            print(f"phone-core: cloned core {key} from {STORE}; no build needed.", file=sys.stderr)
        else:
            if os.path.exists(stamp):
                os.unlink(stamp)
            code = subprocess.run(args.command).returncode
            if code:
                return code
            draft = f"{entry}.new"
            shutil.rmtree(draft, ignore_errors=True)
            for path in args.keep:
                clone(os.path.join(out, path), os.path.join(draft, path))
            os.rename(draft, entry)
            evict()
            print(f"phone-core: stored core {key} for the other checkouts.", file=sys.stderr)
        os.makedirs(os.path.dirname(stamp), exist_ok=True)
        with open(stamp, "w") as file:
            file.write(f"{key}\n")
        return 0
    finally:
        lock.close()


sys.exit(main())
