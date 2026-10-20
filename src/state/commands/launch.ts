import { selectedAgentRuntimeProfiles, selectedProviderProfile } from "../../agents/agentProfiles";
import { fetchResource, peekResource } from "../resources";
import { agentCatalogR } from "../resources.defs";
import { agentWindowId, nearestAgentId, selectTabRefs } from "../selectors";
import { getState, useStore } from "../store";
import { notify } from "../toast";
import { addAgent } from "./agents";
import { newBrowserTab, openDeskSimulator, toggleDesk } from "./desk";
import { createCommandSession, newSshTerminal } from "./sessions";
import { newWindow, selectTab, selectWindowId } from "./tabs";
import { makeWindow, withActiveSession } from "./shared";
import { takeSpareShell, type SpareShell } from "../../terminal/spareShell";
import type { PaneNode } from "../types";
import { openAgentPalette, openPicker } from "./ui";

/** Starts the agent launched last, or the first one installed, in the project in front. */
export async function startAgent(): Promise<void> {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    if (session?.kind !== "project") {
        continueInPickedProject(() => void startAgent());
        return;
    }
    const runtime = selectedAgentRuntimeProfiles(st.providerProfiles, st.selectedProviderProfileIds);
    const catalog = peekResource(agentCatalogR, runtime) ?? (await fetchResource(agentCatalogR, runtime).catch(() => []));
    const installed = catalog.filter((agent) => agent.available !== false);
    const agent = installed.find((candidate) => candidate.type === getState().lastAgentType) ?? installed[0];
    if (!agent) {
        openAgentPalette();
        return;
    }
    addAgent(agent.type, undefined, undefined, {
        profileId: selectedProviderProfile(agent.type, st.providerProfiles, st.selectedProviderProfileIds)?.id,
        detectedExecutablePath: agent.command,
        cwd: session.cwd,
        sessionId: session.id,
    });
}

/** The agent picker, after a project to hold it when there is none in front. */
export function chooseAgent(): void {
    const st = getState();
    if (st.sessions[st.activeSessionId]?.kind === "project") openAgentPalette();
    else continueInPickedProject(openAgentPalette);
}

/** Agents live in projects, so asking for one elsewhere picks a project first and carries on there. */
function continueInPickedProject(then: () => void): void {
    const before = getState().activeSessionId;
    openPicker("projects");
    const stop = useStore.subscribe((st) => {
        if (st.activeSessionId !== before) {
            stop();
            if (st.sessions[st.activeSessionId]?.kind === "project") then();
        } else if (!st.pickerOpen) {
            stop();
        }
    });
}

/** A shell where the person is: a tab in this project or command session, a new login on this host, or a fresh command session. */
export function newTerminal(): void {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    if (session?.kind === "project" || session?.kind === "command") {
        const spare = takeSpareShell(session);
        if (spare) openSpareShell(spare);
        else newWindow();
    } else if (session?.kind === "ssh") newSshTerminal(session.name);
    else createCommandSession();
}

function openSpareShell(spare: SpareShell): void {
    withActiveSession((d, session) => {
        const made = makeWindow(session.cwd, "Terminal");
        const pane: PaneNode = { ...(made.root as PaneNode), id: spare.paneId, ptyId: spare.ptyId };
        d.windows[spare.windowId] = { ...made, id: spare.windowId, root: pane, activePaneId: pane.id };
        d.windowsBySession[session.id] = [...(d.windowsBySession[session.id] ?? []), spare.windowId];
        d.sessions[session.id].activeWindowId = spare.windowId;
        d.zoomedPaneId = null;
    });
}

function bringAgentForward(): string | null {
    const agentId = nearestAgentId(getState());
    if (!agentId) {
        notify("info", "Start an agent first — the desk and its browser belong to one");
        return null;
    }
    const windowId = agentWindowId(getState(), agentId);
    if (windowId) selectWindowId(windowId);
    return agentId;
}

export function newDeskBrowserTab(): void {
    const agentId = bringAgentForward();
    if (agentId) newBrowserTab(agentId);
}

export function newDeskSimulator(): void {
    const agentId = bringAgentForward();
    if (agentId) openDeskSimulator(agentId);
}

export function toggleNearestDesk(): void {
    const agentId = bringAgentForward();
    if (agentId) toggleDesk(agentId);
}

/** The tab at `position` in the strip, counting from one; nine is always the last. */
export function selectTabAt(position: number): void {
    const st = getState();
    const refs = selectTabRefs(st, st.activeSessionId);
    const ref = position >= 9 ? refs.at(-1) : refs[position - 1];
    if (ref) selectTab(ref);
}
