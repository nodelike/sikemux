import asyncio
import json
import os
import re
import sqlite3
import sys
from contextlib import closing
from pathlib import Path
from urllib.request import Request, urlopen

os.environ.setdefault("ANONYMIZED_TELEMETRY", "false")
os.environ.setdefault("BROWSER_USE_CLOUD_SYNC", "false")
os.environ.setdefault("BROWSER_USE_LOGGING_LEVEL", "critical")
os.environ.setdefault("BROWSER_USE_SETUP_LOGGING", "false")

from browser_use.browser import BrowserProfile, BrowserSession
from browser_use.browser.events import CloseTabEvent, NavigateToUrlEvent, SwitchTabEvent
from browser_use.filesystem.file_system import FileSystem
from browser_use.mcp.server import BrowserUseServer, MCP_AVAILABLE
from browser_use.telemetry import MCPServerTelemetryEvent
from browser_use.tools.service import Tools
from browser_use.utils import get_browser_use_version
import mcp.types as mcp_types
import sikemux_harness


HIDDEN_TOOLS = {
    "retry_with_browser_use_agent",
    "browser_list_sessions",
    "browser_close_session",
    "browser_close_all",
    "browser_extract_content",
}


class SikemuxBrowserServer(BrowserUseServer):
    def __init__(self):
        self.agent_id = required_env("SIKEMUX_BROWSER_AGENT_ID")
        if not re.fullmatch(r"[A-Za-z0-9_:-]{1,128}", self.agent_id):
            raise SystemExit("Invalid SIKEMUX_BROWSER_AGENT_ID")
        self.state_dir = Path(required_env("SIKEMUX_BROWSER_STATE_DIR"))
        self.cdp_url = os.environ.get("SIKEMUX_BROWSER_CDP_URL", "").strip() or None
        self.broker_url = os.environ.get("SIKEMUX_BROWSER_BROKER_URL", "").strip() or None
        self.broker_token = os.environ.get("SIKEMUX_BROWSER_BROKER_TOKEN", "").strip() or None
        if not self.cdp_url and not (self.broker_url and self.broker_token):
            raise SystemExit("Missing Sikemux browser endpoint; launch this MCP through Sikemux.")
        self.database = self.state_dir / "tabs.sqlite3"
        self.active_target_id: str | None = None
        super().__init__(session_timeout_minutes=24 * 60)
        self._filter_tools()

    def _filter_tools(self) -> None:
        original = self.server.request_handlers[mcp_types.ListToolsRequest]

        async def filtered(request):
            result = await original(request)
            tools = [tool for tool in result.root.tools if tool.name not in HIDDEN_TOOLS]
            tools.extend(sikemux_harness.tool_definitions())
            return mcp_types.ServerResult(result.root.model_copy(update={"tools": tools}))

        self.server.request_handlers[mcp_types.ListToolsRequest] = filtered

    async def _init_browser_session(self, allowed_domains: list[str] | None = None, **kwargs):
        if self.browser_session:
            return
        cdp_url = self.cdp_url or await asyncio.to_thread(self._request_cdp_url)
        profile = BrowserProfile(
            cdp_url=cdp_url,
            is_local=False,
            keep_alive=True,
            headless=True,
            wait_between_actions=0.05,
            device_scale_factor=1.0,
            disable_security=False,
            downloads_path=str(self.state_dir / "downloads"),
            allowed_domains=allowed_domains,
            **kwargs,
        )
        self.browser_session = BrowserSession(browser_profile=profile)
        await self.browser_session.start()
        self._track_session(self.browser_session)
        self.tools = Tools()
        self.file_system = FileSystem(base_dir=self.state_dir / "files")

    def _request_cdp_url(self) -> str:
        assert self.broker_url and self.broker_token
        request = Request(
            f"{self.broker_url.rstrip('/')}/cdp",
            method="POST",
            headers={"Authorization": f"Bearer {self.broker_token}"},
        )
        with urlopen(request, timeout=20) as response:
            value = json.loads(response.read())
        cdp_url = value.get("cdpUrl")
        if not isinstance(cdp_url, str) or not cdp_url:
            raise RuntimeError("Sikemux browser broker returned no CDP endpoint")
        self.cdp_url = cdp_url
        return cdp_url

    async def _execute_tool(self, tool_name: str, arguments: dict):
        if tool_name in sikemux_harness.METHODS:
            return await sikemux_harness.execute(tool_name, arguments)
        if tool_name in HIDDEN_TOOLS:
            return "This browser tool is disabled by Sikemux."
        if tool_name.startswith("browser_") and tool_name not in {
            "browser_list_sessions",
            "browser_close_session",
            "browser_close_all",
        }:
            if not self.browser_session:
                await self._init_browser_session()
            await self._ensure_owned_tab()
        return await super()._execute_tool(tool_name, arguments)

    async def _ensure_owned_tab(self) -> str:
        assert self.browser_session
        tabs = await self.browser_session.get_tabs()
        owned = self._owned_target_ids()
        available = {tab.target_id for tab in tabs}
        self._prune_targets(available)
        owned &= available
        requested = self._read_active()
        if requested in owned:
            self.active_target_id = requested
        if self.active_target_id not in owned:
            self.active_target_id = next(iter(owned), None)
        if self.active_target_id is None:
            for tab in tabs:
                if tab.url in {"about:blank", "chrome://newtab/"} and self._claim_target(tab.target_id):
                    self.active_target_id = tab.target_id
                    break
        if self.active_target_id is None:
            before = {tab.target_id for tab in tabs}
            event = self.browser_session.event_bus.dispatch(NavigateToUrlEvent(url="about:blank", new_tab=True))
            await event
            tabs = await self.browser_session.get_tabs()
            created = [tab.target_id for tab in tabs if tab.target_id not in before]
            self.active_target_id = created[0] if created else self.browser_session.agent_focus_target_id
            if not self.active_target_id:
                raise RuntimeError("Browser did not create a tab")
            self._register_target(self.active_target_id)
        if self.browser_session.agent_focus_target_id != self.active_target_id:
            event = self.browser_session.event_bus.dispatch(SwitchTabEvent(target_id=self.active_target_id))
            await event
        self._write_active(self.active_target_id)
        return self.active_target_id

    async def _navigate(self, url: str, new_tab: bool = False) -> str:
        assert self.browser_session
        before = {tab.target_id for tab in await self.browser_session.get_tabs()}
        result = await super()._navigate(url, new_tab)
        if new_tab:
            tabs = await self.browser_session.get_tabs()
            created = [tab.target_id for tab in tabs if tab.target_id not in before]
            target_id = created[0] if created else self.browser_session.agent_focus_target_id
            if target_id:
                self._register_target(target_id)
                self.active_target_id = target_id
                self._write_active(target_id)
        return result

    async def _click(self, index=None, coordinate_x=None, coordinate_y=None, new_tab=False):
        assert self.browser_session
        before = {tab.target_id for tab in await self.browser_session.get_tabs()}
        result = await super()._click(index=index, coordinate_x=coordinate_x, coordinate_y=coordinate_y, new_tab=new_tab)
        tabs = await self.browser_session.get_tabs()
        created = [tab.target_id for tab in tabs if tab.target_id not in before]
        for target_id in created:
            self._register_target(target_id)
            self.active_target_id = target_id
            self._write_active(target_id)
        return result

    async def _list_tabs(self) -> str:
        assert self.browser_session
        owned = self._owned_target_ids()
        tabs = [
            {"tab_id": tab.target_id[-4:], "url": tab.url, "title": tab.title or ""}
            for tab in await self.browser_session.get_tabs()
            if tab.target_id in owned
        ]
        return json.dumps(tabs, indent=2)

    async def _switch_tab(self, tab_id: str) -> str:
        assert self.browser_session
        target_id = self._resolve_owned_tab(tab_id)
        event = self.browser_session.event_bus.dispatch(SwitchTabEvent(target_id=target_id))
        await event
        self.active_target_id = target_id
        self._write_active(target_id)
        state = await self.browser_session.get_browser_state_summary()
        return f"Switched to tab {tab_id}: {state.url}"

    async def _close_tab(self, tab_id: str) -> str:
        assert self.browser_session
        target_id = self._resolve_owned_tab(tab_id)
        event = self.browser_session.event_bus.dispatch(CloseTabEvent(target_id=target_id))
        await event
        self._unregister_target(target_id)
        self.active_target_id = None
        owned = self._owned_target_ids()
        if owned:
            await self._ensure_owned_tab()
        else:
            self._write_active(None)
        return f"Closed tab {tab_id}"

    def _connect_registry(self) -> sqlite3.Connection:
        connection = sqlite3.connect(self.database, timeout=5)
        connection.execute(
            "CREATE TABLE IF NOT EXISTS browser_tabs (target_id TEXT PRIMARY KEY, agent_id TEXT NOT NULL, created_at INTEGER NOT NULL DEFAULT (unixepoch()))"
        )
        connection.execute(
            "CREATE TABLE IF NOT EXISTS browser_active_tabs (agent_id TEXT PRIMARY KEY, target_id TEXT NOT NULL)"
        )
        return connection

    def _owned_target_ids(self) -> set[str]:
        with closing(self._connect_registry()) as connection, connection:
            return {row[0] for row in connection.execute("SELECT target_id FROM browser_tabs WHERE agent_id = ?", (self.agent_id,))}

    def _claim_target(self, target_id: str) -> bool:
        with closing(self._connect_registry()) as connection, connection:
            cursor = connection.execute(
                "INSERT OR IGNORE INTO browser_tabs (target_id, agent_id) VALUES (?, ?)",
                (target_id, self.agent_id),
            )
            if cursor.rowcount == 1:
                return True
            owner = connection.execute("SELECT agent_id FROM browser_tabs WHERE target_id = ?", (target_id,)).fetchone()
            return bool(owner and owner[0] == self.agent_id)

    def _register_target(self, target_id: str) -> None:
        with closing(self._connect_registry()) as connection, connection:
            connection.execute(
                "INSERT OR REPLACE INTO browser_tabs (target_id, agent_id) VALUES (?, ?)",
                (target_id, self.agent_id),
            )

    def _unregister_target(self, target_id: str) -> None:
        with closing(self._connect_registry()) as connection, connection:
            connection.execute("DELETE FROM browser_tabs WHERE target_id = ? AND agent_id = ?", (target_id, self.agent_id))

    def _prune_targets(self, available: set[str]) -> None:
        with closing(self._connect_registry()) as connection, connection:
            owned = [row[0] for row in connection.execute("SELECT target_id FROM browser_tabs WHERE agent_id = ?", (self.agent_id,))]
            connection.executemany(
                "DELETE FROM browser_tabs WHERE target_id = ? AND agent_id = ?",
                [(target_id, self.agent_id) for target_id in owned if target_id not in available],
            )

    def _resolve_owned_tab(self, tab_id: str) -> str:
        matches = [target_id for target_id in self._owned_target_ids() if target_id == tab_id or target_id.endswith(tab_id)]
        if len(matches) != 1:
            raise ValueError("Tab does not belong to this Sikemux agent session")
        return matches[0]

    def _write_active(self, target_id: str | None) -> None:
        with closing(self._connect_registry()) as connection, connection:
            if target_id is None:
                connection.execute("DELETE FROM browser_active_tabs WHERE agent_id = ?", (self.agent_id,))
            else:
                connection.execute(
                    "INSERT INTO browser_active_tabs (agent_id, target_id) VALUES (?, ?) "
                    "ON CONFLICT(agent_id) DO UPDATE SET target_id = excluded.target_id",
                    (self.agent_id, target_id),
                )

    def _read_active(self) -> str | None:
        with closing(self._connect_registry()) as connection, connection:
            row = connection.execute(
                "SELECT target_id FROM browser_active_tabs WHERE agent_id = ?", (self.agent_id,)
            ).fetchone()
        return row[0] if row else None


def required_env(name: str) -> str:
    value = os.environ.get(name, "").strip()
    if not value:
        print(f"Missing {name}; launch this MCP through Sikemux.", file=sys.stderr)
        raise SystemExit(2)
    return value


async def main() -> None:
    if not MCP_AVAILABLE:
        print("Browser Use MCP dependencies are missing.", file=sys.stderr)
        raise SystemExit(1)
    server = SikemuxBrowserServer()
    server._telemetry.capture(
        MCPServerTelemetryEvent(version=get_browser_use_version(), action="start", parent_process_cmdline="sikemux")
    )
    try:
        await server.run()
    finally:
        server._telemetry.flush()


if __name__ == "__main__":
    asyncio.run(main())
