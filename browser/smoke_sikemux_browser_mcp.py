import argparse
import asyncio
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client


class SmokePage(BaseHTTPRequestHandler):
    def do_GET(self):
        body = b"<html><head><title>Sikemux Browser Smoke</title></head><body><button>ready</button></body></html>"
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        pass


def wait_for_cdp(path: Path) -> str:
    for _ in range(200):
        try:
            port = path.read_text().splitlines()[0]
            return f"http://127.0.0.1:{port}"
        except (OSError, IndexError):
            time.sleep(0.1)
    raise RuntimeError("Chromium did not expose a CDP endpoint")


def run_agent_smoke(command: list[str], environment: dict[str, str], label: str) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(command, env=environment, text=True, capture_output=True, timeout=30)
    if result.returncode != 0:
        raise RuntimeError(f"{label} browser smoke failed: {(result.stderr or result.stdout).strip()[:1000]}")
    return result


def exercise_agent_hosts(args, cdp_url: str, root: Path) -> None:
    agent_state = root / "agent-state"
    agent_state.mkdir()
    base_environment = {
        **os.environ,
        "SIKEMUX_BROWSER_STATE_DIR": str(agent_state),
        "SIKEMUX_BROWSER_CDP_URL": cdp_url,
        "SIKEMUX_BROWSER_AGENT_ID": "agent-host-smoke",
    }
    if args.hermes:
        hermes_home = root / "hermes"
        hermes_home.mkdir()
        hermes_home.joinpath("config.yaml").write_text(
            "mcp_servers:\n"
            "  sikemux_browser:\n"
            f"    command: {json.dumps(str(args.sidecar))}\n"
            "    args: []\n"
            "    enabled: true\n"
        )
        result = run_agent_smoke(
            [str(args.hermes), "mcp", "test", "sikemux_browser"],
            {**base_environment, "HERMES_HOME": str(hermes_home)},
            "Hermes",
        )
        if "sikemux_browser" not in result.stdout:
            raise RuntimeError("Hermes MCP probe did not report sikemux_browser")
    if args.pi and args.pi_extension:
        result = run_agent_smoke(
            [str(args.pi), "--extension", str(args.pi_extension), "--list-models", "sikemux-no-model-match"],
            {
                **base_environment,
                "SIKEMUX_BROWSER_MCP_COMMAND": str(args.sidecar),
                "SIKEMUX_BROWSER_MCP_ARGS": "[]",
            },
            "Pi",
        )
        if "[sikemux-browser]" in result.stderr:
            raise RuntimeError(f"Pi browser extension failed: {result.stderr.strip()[:1000]}")
    if args.opencode:
        config = {
            "mcp": {
                "sikemux_browser": {
                    "type": "local",
                    "command": [str(args.sidecar)],
                    "enabled": True,
                }
            }
        }
        result = run_agent_smoke(
            [str(args.opencode), "mcp", "list"],
            {**base_environment, "OPENCODE_CONFIG_CONTENT": json.dumps(config)},
            "OpenCode",
        )
        if "sikemux_browser" not in result.stdout:
            raise RuntimeError("OpenCode MCP probe did not report sikemux_browser")
    if args.omp and args.pi_extension:
        result = run_agent_smoke(
            [str(args.omp), "models", "--extension", str(args.pi_extension), "sikemux-no-model-match"],
            {
                **base_environment,
                "SIKEMUX_BROWSER_MCP_COMMAND": str(args.sidecar),
                "SIKEMUX_BROWSER_MCP_ARGS": "[]",
            },
            "OMP",
        )
        if "[sikemux-browser]" in result.stderr:
            raise RuntimeError(f"OMP browser extension failed: {result.stderr.strip()[:1000]}")
    if args.grok:
        grok_home = root / "grok"
        grok_home.mkdir()
        grok_environment = {**base_environment, "GROK_HOME": str(grok_home)}
        run_agent_smoke(
            [str(args.grok), "mcp", "add", "sikemux_browser", "--", str(args.sidecar)],
            grok_environment,
            "Grok config",
        )
        result = run_agent_smoke(
            [str(args.grok), "mcp", "doctor", "sikemux_browser", "--json"],
            grok_environment,
            "Grok",
        )
        if "sikemux_browser" not in result.stdout:
            raise RuntimeError("Grok MCP probe did not report sikemux_browser")


async def exercise_sidecar(sidecar: Path, cdp_url: str, state_dir: Path, page_url: str) -> None:
    environment = {
        **os.environ,
        "SIKEMUX_BROWSER_AGENT_ID": "smoke-agent",
        "SIKEMUX_BROWSER_STATE_DIR": str(state_dir),
        "SIKEMUX_BROWSER_CDP_URL": cdp_url,
    }
    parameters = StdioServerParameters(command=str(sidecar), env=environment)
    async with stdio_client(parameters, errlog=sys.stderr) as (reader, writer):
        async with ClientSession(reader, writer) as session:
            await session.initialize()
            tools = {tool.name for tool in (await session.list_tools()).tools}
            required = {"browser_navigate", "browser_get_state", "browser_list_tabs", "sikemux_workspace_inspect", "sikemux_task_start", "sikemux_task_read", "sikemux_task_stop", "sikemux_ui_open", "sikemux_events_wait"}
            if not required <= tools:
                raise RuntimeError(f"frozen sidecar is missing tools: {sorted(required - tools)}")
            if "browser_extract_content" in tools:
                raise RuntimeError("frozen sidecar exposes browser_extract_content without an extraction LLM")

            navigate = await session.call_tool("browser_navigate", {"url": page_url})
            if navigate.isError:
                raise RuntimeError(f"browser_navigate failed: {navigate.content}")
            text = ""
            for _ in range(50):
                state = await session.call_tool("browser_get_state", {})
                text = "\n".join(getattr(block, "text", "") for block in state.content)
                if page_url in text and '"text": "ready"' in text:
                    break
                await asyncio.sleep(0.1)
            else:
                raise RuntimeError(f"browser state did not contain the smoke page: {text[:500]}")
            tabs = await session.call_tool("browser_list_tabs", {})
            tab_text = "\n".join(getattr(block, "text", "") for block in tabs.content)
            if page_url not in tab_text:
                raise RuntimeError("owned tab list did not contain the smoke page")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--sidecar", type=Path, required=True)
    parser.add_argument("--browser", type=Path, required=True)
    parser.add_argument("--hermes", type=Path)
    parser.add_argument("--pi", type=Path)
    parser.add_argument("--pi-extension", type=Path)
    parser.add_argument("--opencode", type=Path)
    parser.add_argument("--omp", type=Path)
    parser.add_argument("--grok", type=Path)
    args = parser.parse_args()
    if not args.sidecar.is_file() or not args.browser.is_file():
        raise SystemExit("sidecar or browser executable is missing")

    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        profile = root / "profile"
        state = root / "state"
        profile.mkdir()
        state.mkdir()
        server = ThreadingHTTPServer(("127.0.0.1", 0), SmokePage)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        browser = subprocess.Popen(
            [
                str(args.browser),
                "--headless=new",
                "--remote-debugging-address=127.0.0.1",
                "--remote-debugging-port=0",
                "--no-first-run",
                "--no-default-browser-check",
                f"--user-data-dir={profile}",
                "about:blank",
            ],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        try:
            cdp_url = wait_for_cdp(profile / "DevToolsActivePort")
            page_url = f"http://127.0.0.1:{server.server_port}/"
            asyncio.run(exercise_sidecar(args.sidecar, cdp_url, state, page_url))
            exercise_agent_hosts(args, cdp_url, root)
        finally:
            browser.terminate()
            try:
                browser.wait(timeout=5)
            except subprocess.TimeoutExpired:
                browser.kill()
                browser.wait()
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)
    print("Browser sidecar smoke test passed")


if __name__ == "__main__":
    main()
