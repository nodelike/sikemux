import { agentLabel, isAgentType, type AgentType } from "@shared/agents/catalog";
import { isResumeId } from "@shared/agents/agentLaunch";
import { appShareLink, parseShareLink, type ShareLink } from "@shared/share/link";
import { confirmDialog } from "../dialog";
import { collectPanes } from "../layout";
import { projectSessionId } from "../selectors";
import { getState, mutate } from "../store";
import type { Agent, Session } from "../types";
import { addAgent } from "./agents";
import { makeWindow } from "./shared";
import { selectWindowId } from "./tabs";
import { createProjectSession } from "./sessions";

/**
 * A link that brings this agent's conversation back up, in its project, from
 * anywhere on the Mac. Null until the agent has a conversation to come back to.
 */
export function agentLink(agent: Agent, session: Session): string | null {
    if (!agent.resumeId || session.kind !== "project") return null;
    return `sikemux://agent/${agent.type}/${encodeURIComponent(agent.resumeId)}?project=${encodeURIComponent(session.cwd)}`;
}

/**
 * Opens what a sikemux:// link points at, and says whether it understood the link.
 * Any page or message can hand the app a link, so one naming a folder that is not
 * already a project asks the person before an agent starts there.
 */
export function routeDeepLink(link: string): boolean {
    let url: URL;
    try {
        url = new URL(link);
    } catch {
        return false;
    }
    const share = parseShareLink(link);
    if (share) return openSharedChat(share);
    if (url.protocol !== "sikemux:" || url.hostname !== "agent") return false;
    const [type, resumeId] = url.pathname.split("/").filter(Boolean).map(decodeURIComponent);
    const project = url.searchParams.get("project");
    if (!type || !isAgentType(type) || !resumeId || !isResumeId(type, resumeId) || !project?.startsWith("/")) return false;
    if (projectSessionId(getState(), project)) return resumeInProject(type, resumeId, project);
    void confirmDialog({
        title: "Open this folder as a project?",
        body: `A link asks to resume a ${agentLabel(type)} chat in ${project}, which is not one of your projects.\nThe agent starts there at once and follows the folder's own settings and hooks. Go ahead only if you trust it.`,
        confirmLabel: "Open project",
    }).then((confirmed) => {
        if (confirmed) resumeInProject(type, resumeId, project);
    });
    return true;
}

function resumeInProject(type: AgentType, resumeId: string, project: string): boolean {
    createProjectSession(project);
    return addAgent(type, resumeId);
}

/** Opens a chat someone shared, in a tab of the session in front, or goes to the tab already showing it. */
export function openSharedChat(share: ShareLink): boolean {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    if (!session) return false;
    const startup = appShareLink(share);
    const ids = st.windowsBySession[session.id] ?? [];
    const existing = ids.find((id) => {
        const win = st.windows[id];
        return win && collectPanes(win.root).some((pane) => pane.kind === "shared" && pane.startup === startup);
    });
    if (existing) {
        selectWindowId(existing);
        return true;
    }
    mutate((d) => {
        const win = makeWindow(session.cwd, "Shared chat", { kind: "shared", role: "shared", startup });
        d.windows[win.id] = win;
        d.windowsBySession[session.id] = [...(d.windowsBySession[session.id] ?? []), win.id];
        d.sessions[session.id].activeWindowId = win.id;
        d.zoomedPaneId = null;
    });
    return true;
}
