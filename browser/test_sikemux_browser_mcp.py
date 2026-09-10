import asyncio
import json
import os
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from unittest.mock import AsyncMock, patch

from mcp.types import ListToolsRequest

from sikemux_browser_mcp import HIDDEN_TOOLS, SikemuxBrowserServer


class SikemuxBrowserServerTests(unittest.TestCase):
    def server(self, agent_id: str, state_dir: Path) -> SikemuxBrowserServer:
        os.environ.update(
            SIKEMUX_BROWSER_AGENT_ID=agent_id,
            SIKEMUX_BROWSER_STATE_DIR=str(state_dir),
            SIKEMUX_BROWSER_CDP_URL="http://127.0.0.1:1",
        )
        return SikemuxBrowserServer()

    def test_registry_isolates_agent_tabs(self):
        with tempfile.TemporaryDirectory() as directory:
            state_dir = Path(directory)
            first = self.server("agent-one", state_dir)
            second = self.server("agent-two", state_dir)

            first._register_target("target-one")
            second._register_target("target-two")
            first._write_active("target-one")
            second._write_active("target-two")

            self.assertEqual(first._owned_target_ids(), {"target-one"})
            self.assertEqual(second._owned_target_ids(), {"target-two"})
            self.assertEqual(first._read_active(), "target-one")
            self.assertEqual(second._read_active(), "target-two")
            with self.assertRaises(ValueError):
                first._resolve_owned_tab("two")

    def test_tool_list_exposes_only_scoped_direct_controls(self):
        with tempfile.TemporaryDirectory() as directory:
            server = self.server("agent-one", Path(directory))

            async def list_tools():
                handler = server.server.request_handlers[ListToolsRequest]
                result = await handler(ListToolsRequest(method="tools/list"))
                return {tool.name for tool in result.root.tools}

            tools = asyncio.run(list_tools())
            self.assertFalse(tools & HIDDEN_TOOLS)
            self.assertIn("sikemux_workspace_inspect", tools)
            self.assertIn("sikemux_events_wait", tools)
            self.assertIn("browser_get_state", tools)
            self.assertIn("browser_switch_tab", tools)
            self.assertNotIn("browser_extract_content", tools)

    def test_harness_tool_does_not_start_chromium(self):
        with tempfile.TemporaryDirectory() as directory:
            server = self.server("agent-one", Path(directory))
            with patch("sikemux_browser_mcp.sikemux_harness.execute", new_callable=AsyncMock, return_value='{"ok":true}') as execute, patch.object(server, "_init_browser_session", new_callable=AsyncMock) as start:
                result = asyncio.run(server._execute_tool("sikemux_workspace_inspect", {}))
                self.assertEqual(result, '{"ok":true}')
                execute.assert_awaited_once_with("sikemux_workspace_inspect", {})
                start.assert_not_called()

    def test_rejects_agent_ids_that_can_escape_state_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(SystemExit):
                self.server("../agent", Path(directory))

    def test_requests_cdp_from_authenticated_lazy_broker(self):
        received = []

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                received.append((self.path, self.headers.get("Authorization")))
                body = json.dumps({"cdpUrl": "http://127.0.0.1:9222"}).encode()
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *_args):
                pass

        broker = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=broker.serve_forever, daemon=True)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as directory:
                os.environ.pop("SIKEMUX_BROWSER_CDP_URL", None)
                os.environ.update(
                    SIKEMUX_BROWSER_AGENT_ID="agent-one",
                    SIKEMUX_BROWSER_STATE_DIR=directory,
                    SIKEMUX_BROWSER_BROKER_URL=f"http://127.0.0.1:{broker.server_port}",
                    SIKEMUX_BROWSER_BROKER_TOKEN="secret",
                )
                server = SikemuxBrowserServer()
                self.assertEqual(server._request_cdp_url(), "http://127.0.0.1:9222")
                self.assertEqual(received, [("/cdp", "Bearer secret")])
        finally:
            broker.shutdown()
            broker.server_close()
            thread.join(timeout=5)


if __name__ == "__main__":
    unittest.main()
