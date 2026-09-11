import { act, cleanup, render } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { Workspace } from "./Workspace";
import { getState, setState, type StoreState } from "../state/store";
import { selectSession, selectTab } from "../state/commands";
import type { PaneNode } from "../state/types";

vi.mock("../terminal/TerminalPane", () => ({ TerminalPane: () => <div>Terminal output</div> }));
vi.mock("../chat/AgentSurface", () => ({ AgentSurface: () => <div>Agent output</div> }));
vi.mock("./BrowserPane", () => ({ AgentBrowserShell: ({ children }: { children: React.ReactNode }) => children }));
vi.mock("./EditorPane", () => ({ EditorPane: () => <div>Editor document</div> }));
vi.mock("./GitPane", () => ({ GitPane: () => <div>Git status</div> }));

const initial = getState();
afterEach(() => {
    cleanup();
    setState(initial, true);
});

function fixture(projects: number): StoreState {
    const state: StoreState = {
        ...initial,
        sessions: {},
        windows: {},
        agents: {},
        editorViews: {},
        sessionOrder: [],
        windowsBySession: {},
        agentsBySession: {},
        activeSessionId: "project-0",
    };
    for (let index = 0; index < projects; index++) {
        const id = `project-${index}`;
        const cwd = `/repo/${id}`;
        state.sessionOrder.push(id);
        state.windowsBySession[id] = [];
        state.agentsBySession[id] = [];
        for (let tab = 0; tab < 12; tab++) {
            const windowId = `${id}-window-${tab}`;
            const pane: PaneNode = {
                type: "pane",
                id: `${windowId}-pane`,
                cwd,
                kind: tab === 10 ? "editor" : tab === 11 ? "git" : "terminal",
                title: windowId,
            };
            state.windows[windowId] = {
                id: windowId,
                name: String(tab + 1),
                role: tab === 10 ? "files" : tab === 11 ? "git" : "term",
                root: pane,
                activePaneId: pane.id,
            };
            state.windowsBySession[id].push(windowId);
            if (tab === 10) {
                const openTabs = Array.from({ length: 100 }, (_, file) => `${cwd}/file-${file}.ts`);
                state.editorViews[pane.id] = { openTabs, activePath: openTabs[0] };
            }
        }
        for (let tab = 0; tab < 5; tab++) {
            const agentId = `${id}-agent-${tab}`;
            state.agents[agentId] = { id: agentId, type: "codex", title: agentId, startup: "codex", launchState: "live" };
            state.agentsBySession[id].push(agentId);
        }
        state.sessions[id] = {
            id,
            name: id,
            kind: "project",
            cwd,
            deploy: null,
            pinned: false,
            activeWindowId: state.windowsBySession[id][0],
            activeAgentId: null,
            view: "windows",
        };
    }
    return state;
}

for (const projects of [1, 10, 50]) {
    it(`switches mixed tabs with ${projects} projects, ${projects * 100} file tabs, and ${projects * 15} terminal/agent tabs`, async () => {
        setState(fixture(projects), true);
        const { container } = render(<Workspace />);
        await act(async () => {});
        const samples: number[] = [];
        for (let index = 0; index < 100; index++) {
            const projectId = `project-${index % projects}`;
            const start = performance.now();
            await act(async () => {
                selectSession(projectId);
                if (index % 4 === 0) selectTab({ kind: "agent", id: `${projectId}-agent-${index % 5}` });
                else if (index % 4 === 1)
                    selectTab({ kind: "file", id: `${projectId}-window-10`, path: `/repo/${projectId}/file-${index % 100}.ts` });
                else selectTab({ kind: "window", id: `${projectId}-window-${index % 4 === 2 ? 11 : index % 10}` });
            });
            samples.push(performance.now() - start);
            expect(getState().activeSessionId).toBe(projectId);
            expect(container.querySelectorAll('[role="tabpanel"][aria-hidden="false"]')).toHaveLength(1);
        }
        samples.sort((a, b) => a - b);
        console.info(
            JSON.stringify({
                projects,
                fileTabs: projects * 100,
                terminalTabs: projects * 10,
                agentTabs: projects * 5,
                switches: samples.length,
                medianMs: samples[49],
                p95Ms: samples[94],
                maxMs: samples[99],
                renderers: "mocked",
            }),
        );
    }, 30_000);
}
