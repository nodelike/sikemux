#!/usr/bin/env bash
# Rehearses the update a Linux host takes by itself: installs the last stable host core with
# its own installer, then has that build's root updater fetch, check and put in place the
# candidate, and checks the core answers again as the candidate. The machine reaches a
# github.com served from this machine, trusted through a certificate authority made here,
# because a published build reads its feed from github.com and nowhere else.
#
# Run it on a throwaway machine, as a person with sudo: it rewrites /etc/hosts and trusts a
# new authority.
# The candidate's signed core is taken from target/host when the run built it, otherwise
# from its GitHub release.
set -euo pipefail

cd "$(dirname "$0")/.."
TO="${1:?usage: $0 <candidate version> [from version]}"
FROM="${2:-$(gh release view -R nodelike/sikemux --json tagName -q .tagName | sed 's/^v//')}"
ARCH="$(uname -m)"
UPDATE_LIMIT=300

fail() {
  echo "Update rehearsal failed: $*" >&2
  [[ -f "$WORK/server.log" ]] && tail -n 30 "$WORK/server.log" >&2
  exit 1
}

as_root() { if [[ "$(id -u)" == 0 ]]; then "$@"; else sudo "$@"; fi; }
WORK="$(mktemp -d)"
WWW="$WORK/www"
RELEASES="$WWW/nodelike/sikemux/releases"
mkdir -p "$RELEASES/download/v$FROM" "$RELEASES/download/v$TO" "$RELEASES/latest/download"

host_core() { echo "sikemux-host-$1-linux-$ARCH.tar.gz"; }

echo "→ Fetching v$FROM, and the candidate v$TO"
gh release download "v$FROM" -R nodelike/sikemux -D "$RELEASES/download/v$FROM" \
  -p install.sh -p "$(host_core "$FROM")" -p "$(host_core "$FROM").sig"
if [[ -f "target/host/$(host_core "$TO").sig" ]]; then
  cp "target/host/$(host_core "$TO")" "target/host/$(host_core "$TO").sig" "$RELEASES/download/v$TO/"
else
  gh release download "v$TO" -R nodelike/sikemux -D "$RELEASES/download/v$TO" \
    -p "$(host_core "$TO")" -p "$(host_core "$TO").sig"
fi

# The feed a stable machine reads, naming only the host core: what root's updater looks for.
feed() {
  local version="$1" file
  file="$(host_core "$version")"
  python3 - "$version" "$ARCH" "$RELEASES/download/v$version/$file.sig" "$file" <<'PY'
import json, pathlib, sys
version, arch, signature, file = sys.argv[1:]
print(json.dumps({
    "version": version,
    "notes": "Update rehearsal",
    "pub_date": "2026-01-01T00:00:00Z",
    "platforms": {f"host-linux-{arch}": {
        "signature": pathlib.Path(signature).read_text().strip(),
        "url": f"https://github.com/nodelike/sikemux/releases/download/v{version}/{file}",
    }},
}, indent=2))
PY
}
latest() {
  echo "$1" >"$WORK/latest"
  feed "$1" >"$RELEASES/latest/download/latest.json"
}
latest "$FROM"

echo "→ Serving github.com from this machine"
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj "/CN=Sikemux update rehearsal" \
  -keyout "$WORK/ca.key" -out "$WORK/ca.crt" -addext basicConstraints=critical,CA:TRUE \
  -addext keyUsage=critical,keyCertSign 2>/dev/null
openssl req -newkey rsa:2048 -nodes -subj "/CN=github.com" -keyout "$WORK/site.key" \
  -out "$WORK/site.csr" 2>/dev/null
openssl x509 -req -in "$WORK/site.csr" -CA "$WORK/ca.crt" -CAkey "$WORK/ca.key" -CAcreateserial \
  -days 1 -out "$WORK/site.crt" -extfile <(printf 'subjectAltName=DNS:github.com\nextendedKeyUsage=serverAuth\n') 2>/dev/null
if command -v update-ca-certificates >/dev/null; then
  as_root cp "$WORK/ca.crt" /usr/local/share/ca-certificates/sikemux-rehearsal.crt
  as_root update-ca-certificates >/dev/null
else
  as_root cp "$WORK/ca.crt" /etc/pki/ca-trust/source/anchors/sikemux-rehearsal.crt
  as_root update-ca-trust
fi
cp /etc/hosts "$WORK/hosts"
trap 'as_root cp "$WORK/hosts" /etc/hosts; as_root kill "$SERVER" 2>/dev/null || true' EXIT
echo "127.0.0.1 github.com" | as_root tee -a /etc/hosts >/dev/null

as_root python3 - "$WWW" "$WORK" >"$WORK/server.log" 2>&1 <<'PY' &
import http.server, pathlib, ssl, sys
www, work = map(pathlib.Path, sys.argv[1:])

class Releases(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(www), **kwargs)

    def do_HEAD(self):
        self.route() or super().do_HEAD()

    def do_GET(self):
        self.route() or super().do_GET()

    def route(self):
        latest = (work / "latest").read_text().strip()
        if self.path == "/nodelike/sikemux/releases/latest":
            self.send_response(302)
            self.send_header("Location", f"/nodelike/sikemux/releases/tag/v{latest}")
            self.end_headers()
            return True
        if self.path.startswith("/nodelike/sikemux/releases/tag/"):
            self.send_response(200)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return True
        return False

server = http.server.ThreadingHTTPServer(("127.0.0.1", 443), Releases)
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(work / "site.crt", work / "site.key")
server.socket = context.wrap_socket(server.socket, server_side=True)
server.serve_forever()
PY
SERVER=$!
for _ in $(seq 50); do
  curl -fsS -o /dev/null https://github.com/nodelike/sikemux/releases/latest/download/latest.json 2>/dev/null && break
  sleep 0.2
done
curl -fsS -o /dev/null https://github.com/nodelike/sikemux/releases/latest/download/latest.json ||
  fail "the local github.com does not answer"

export PATH="/usr/local/bin:$HOME/.local/bin:$PATH"
# The installer starts the core as a systemd user service, which sees only the user manager's
# environment, not this run's CI, so a rehearsal would count as a person's install.
export DO_NOT_TRACK=1
systemctl --user set-environment DO_NOT_TRACK=1 2>/dev/null || true
core_env() {
  local pid
  pid=$(pgrep -n -f '/sikemux-host core' || true)
  [[ -n "$pid" ]] || fail "no core is running"
  as_root cat "/proc/$pid/environ" | tr '\0' '\n' | cut -d= -f1 | sort | tr '\n' ' '
}
untracked() {
  local names
  names=$(core_env)
  echo "  the core's environment holds: $names"
  [[ " $names " == *" DO_NOT_TRACK "* ]] || fail "the core would report usage: DO_NOT_TRACK is not in its environment"
}

echo "→ Installing v$FROM with its own installer"
curl -fsSL -o "$WORK/install.sh" "https://github.com/nodelike/sikemux/releases/download/v$FROM/install.sh" ||
  fail "could not fetch the v$FROM installer from the local github.com"
sh "$WORK/install.sh" --host --yes </dev/null ||
  fail "the v$FROM installer exited with $?"
[[ "$(sikemux --version)" == "sikemux $FROM" ]] || fail "installed $(sikemux --version), not v$FROM"
sikemux start >/dev/null 2>&1 || true

core_version() { sikemux status --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("version") or "")' 2>/dev/null || true; }
for _ in $(seq 30); do [[ "$(core_version)" == "$FROM" ]] && break; sleep 1; done
[[ "$(core_version)" == "$FROM" ]] || fail "the v$FROM core does not answer"
untracked

echo "→ Offering v$TO on the stable feed"
latest "$TO"
began=$SECONDS
sikemux update || echo "  sikemux update exited with $?; the core may already be taking it by itself"
until [[ "$(core_version)" == "$TO" ]]; do
  ((SECONDS - began < UPDATE_LIMIT)) || fail "the core still answers as $(core_version) after ${UPDATE_LIMIT}s"
  sleep 2
done
echo "  the core answers as v$TO after $((SECONDS - began))s"
[[ "$(sikemux --version)" == "sikemux $TO" ]] || fail "sikemux on PATH is $(sikemux --version)"
# A new core that stops within its first start is put back, so it must still answer after that.
sleep 40
[[ "$(core_version)" == "$TO" ]] || fail "after 40s the core answers as $(core_version)"
untracked
echo "✓ v$FROM updated itself to v$TO and the new core stayed up"
