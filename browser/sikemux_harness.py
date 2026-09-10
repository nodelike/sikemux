import asyncio
import json
import os
from pathlib import Path
import socket
import uuid

import mcp.types as types


METHODS = {
    "sikemux_workspace_inspect": ("workspace.inspect", "Inspect this agent's open project, panes, configured tasks, runs, and event cursor.", {}, []),
    "sikemux_task_start": ("task.start", "Start a configured project task in a managed terminal. Reuse the same idempotencyKey when retrying. May require project trust in Sikemux. previewUrl is configuration, not proof of readiness.", {"taskId": {"type": "string"}, "idempotencyKey": {"type": "string", "maxLength": 128}}, ["taskId", "idempotencyKey"]),
    "sikemux_task_read": ("task.read", "Read task status and new terminal output using its byte cursor. Start at cursor 0, then pass back the returned cursor. Read again while hasMore. Output may contain terminal escape sequences.", {"executionId": {"type": "string"}, "cursor": {"type": "integer", "minimum": 0}, "limit": {"type": "integer", "minimum": 4, "maximum": 8192}}, ["executionId"]),
    "sikemux_task_stop": ("task.stop", "Stop the exact managed task execution and its process tree.", {"executionId": {"type": "string"}}, ["executionId"]),
    "sikemux_ui_open": ("ui.open", "Open a project file, diff, task terminal, or configured preview. Background by default; focus=true reveals it. Preview belongs to this agent's browser.", {"kind": {"enum": ["file", "diff", "terminal", "preview"]}, "path": {"type": "string"}, "line": {"type": "integer", "minimum": 1}, "executionId": {"type": "string"}, "focus": {"type": "boolean"}}, ["kind"]),
    "sikemux_events_wait": ("events.wait", "Wait for project task output/lifecycle or UI-open events after a workspace/event cursor. Returns on matching events or timeout; does not schedule future agent turns.", {"cursor": {"type": "string"}, "timeoutMs": {"type": "integer", "minimum": 0, "maximum": 30000}, "executionId": {"type": "string"}}, ["cursor"]),
}


def tool_definitions():
    return [types.Tool(name=name, description=description, inputSchema={"type": "object", "properties": properties, "required": required, "additionalProperties": False}) for name, (_, description, properties, required) in METHODS.items()]


def call_harness(name, arguments):
    method = METHODS[name][0]
    endpoint_path = os.environ.get("SIKEMUX_CLI_ENDPOINT")
    if not endpoint_path:
        raise RuntimeError("Missing SIKEMUX_CLI_ENDPOINT; launch this MCP from Sikemux")
    endpoint = json.loads(Path(endpoint_path).read_text())
    request = {"command": "harness", "protocol": endpoint["protocol"], "token": endpoint["token"], "request": {
        "id": str(uuid.uuid4()), "project": os.environ.get("SIKEMUX_PROJECT") or str(Path.cwd()),
        "agentId": os.environ.get("SIKEMUX_BROWSER_AGENT_ID") or os.environ.get("SIKEMUX_AGENT_ID"),
        "method": method, "params": arguments,
    }}
    frame = json.dumps(request).encode() + b"\n"
    if len(frame) > 65536:
        raise ValueError("Harness request exceeds 64 KiB")
    with socket.create_connection(("127.0.0.1", endpoint["port"]), timeout=5) as connection:
        connection.settimeout(70)
        connection.sendall(frame)
        with connection.makefile("rb") as reader:
            response = reader.readline(65537)
    if len(response) > 65536 or not response.endswith(b"\n"):
        raise RuntimeError("Invalid or oversized harness response")
    result = json.loads(response)
    if result.get("status") == "error":
        raise RuntimeError(result.get("message", "Harness request failed"))
    if result.get("status") != "result":
        raise RuntimeError("Unexpected harness response")
    return json.dumps(result["value"], ensure_ascii=False)


async def execute(name, arguments):
    return await asyncio.to_thread(call_harness, name, arguments)
