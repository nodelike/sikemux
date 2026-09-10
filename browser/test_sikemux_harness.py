import asyncio
import json
import sys
import os
from pathlib import Path
import socket
import tempfile
import threading
import unittest
from unittest.mock import patch

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

from sikemux_harness import call_harness, tool_definitions


class HarnessTests(unittest.TestCase):
    def test_tools_have_bounded_wait_and_required_idempotency(self):
        tools = {tool.name: tool for tool in tool_definitions()}
        self.assertEqual(len(tools), 6)
        self.assertIn("idempotencyKey", tools["sikemux_task_start"].inputSchema["required"])
        self.assertEqual(tools["sikemux_events_wait"].inputSchema["properties"]["timeoutMs"]["maximum"], 30000)

    def exchange(self, response, callback):
        with tempfile.TemporaryDirectory() as directory, socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen(1)
            listener.settimeout(15)
            endpoint = Path(directory) / "endpoint.json"
            endpoint.write_text(json.dumps({"protocol": 1, "token": "test-token", "port": listener.getsockname()[1]}))
            received = []
            def serve():
                connection, _ = listener.accept()
                with connection, connection.makefile("rb") as stream:
                    received.append(json.loads(stream.readline()))
                    connection.sendall(json.dumps(response).encode() + b"\n")
            thread = threading.Thread(target=serve)
            thread.start()
            with patch.dict(os.environ, {"SIKEMUX_CLI_ENDPOINT": str(endpoint), "SIKEMUX_PROJECT": "/project", "SIKEMUX_BROWSER_AGENT_ID": "agent-one"}):
                callback()
            thread.join(timeout=3)
            self.assertFalse(thread.is_alive())
            return received[0]

    def test_authenticated_socket_request_carries_context_and_returns_result(self):
        request = self.exchange({"status": "result", "value": {"project": "/project"}}, lambda: self.assertEqual(json.loads(call_harness("sikemux_workspace_inspect", {})), {"project": "/project"}))
        self.assertEqual(request["token"], "test-token")
        self.assertEqual(request["request"]["method"], "workspace.inspect")
        self.assertEqual(request["request"]["agentId"], "agent-one")
        self.assertEqual(request["request"]["project"], "/project")

    def test_real_stdio_mcp_round_trip_without_chromium(self):
        async def call():
            with tempfile.TemporaryDirectory() as state_dir:
                command = os.environ.get("SIKEMUX_TEST_SIDECAR") or sys.executable
                args = [] if os.environ.get("SIKEMUX_TEST_SIDECAR") else [str(Path(__file__).with_name("sikemux_browser_mcp.py"))]
                env = {**os.environ, "SIKEMUX_BROWSER_STATE_DIR": state_dir, "SIKEMUX_BROWSER_CDP_URL": "http://127.0.0.1:1"}
                async with stdio_client(StdioServerParameters(command=command, args=args, env=env)) as streams:
                    async with ClientSession(*streams) as session:
                        await session.initialize()
                        names = {tool.name for tool in (await session.list_tools()).tools}
                        self.assertIn("sikemux_workspace_inspect", names)
                        result = await session.call_tool("sikemux_workspace_inspect", {})
                        self.assertFalse(result.isError)
                        self.assertEqual(json.loads(result.content[0].text), {"project": "/project"})
        self.exchange({"status": "result", "value": {"project": "/project"}}, lambda: asyncio.run(call()))

    def test_server_error_propagates(self):
        def invoke():
            with self.assertRaisesRegex(RuntimeError, "not open"):
                call_harness("sikemux_workspace_inspect", {})
        self.exchange({"status": "error", "message": "Project not open"}, invoke)

    def test_missing_endpoint_fails_without_connecting(self):
        with patch.dict(os.environ, {}, clear=True), self.assertRaisesRegex(RuntimeError, "SIKEMUX_CLI_ENDPOINT"):
            call_harness("sikemux_workspace_inspect", {})
