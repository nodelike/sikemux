import { portsApi } from "../api/ports";
import { copyText } from "../lib/clipboard";
import * as cmd from "../state/commands";
import { activeAgentId } from "../state/selectors";
import { getState } from "../state/store";
import { notify, reportError } from "../state/toast";
import type { PortReveal } from "./projectPorts";

export function openPortOnDesk(agentId: string, url: string): void {
    const state = getState();
    if (activeAgentId(state, state.sessions[state.activeSessionId]) !== agentId) cmd.revealAgent(agentId);
    cmd.openUrlOnDesk(agentId, url);
}

export function openPortExternally(url: string): void {
    void portsApi.openExternal(url).catch(reportError("open in browser"));
}

export function copyPortUrl(url: string): void {
    copyText(url)
        .then(() => notify("success", `Copied ${url}`))
        .catch(reportError("copy URL"));
}

export function revealPortOwner(reveal: PortReveal): void {
    if (reveal.kind === "agent") {
        cmd.revealAgent(reveal.agentId);
        return;
    }
    if (reveal.kind === "desk-terminal") {
        cmd.revealAgent(reveal.agentId);
        cmd.showDeskTerminal(reveal.agentId, reveal.id);
        return;
    }
    cmd.selectSession(reveal.sessionId);
    cmd.selectWindowId(reveal.windowId);
    cmd.focusPane(reveal.paneId);
}
