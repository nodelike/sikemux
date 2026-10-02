import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WindowRole } from "../state/types";
import { getState, setState } from "../state/store";
import { SideRail } from "./SideRail";
import { TopBar } from "../shell/TopBar";
import { agentWindowId } from "../state/selectors";
import { withAgents } from "../test/agents";

const { gitStatus, gitOverview } = vi.hoisted(() => {
    const gitStatus = {
        branch: "feature/always-visible",
        upstream: "origin/feature/always-visible",
        ahead: 1,
        behind: 0,
        files: [],
    };
    return { gitStatus, gitOverview: { status: gitStatus, branches: [], log: [] } };
});

vi.mock("../state/resources", async (importOriginal) => {
    const actual = await importOriginal<typeof import("../state/resources")>();
    const handle = (kind: string, enabled = true) => ({
        data: enabled && kind === "git.overview" ? gitOverview : undefined,
        status: "ok" as const,
        error: undefined,
        refresh: async () => {},
    });
    return {
        ...actual,
        useResource: (def: { kind: string }) => handle(def.kind),
        useResourceEnabled: (enabled: boolean, def: { kind: string }) => handle(def.kind, enabled),
    };
});

vi.mock("../hooks/useBattery", () => ({ useBattery: () => null }));
vi.mock("../hooks/useClock", () => ({ useClock: () => new Date("2026-08-07T12:00:00") }));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: () => Promise.resolve("test") }));

const initial = getState();

beforeEach(() => {
    setState(initial, true);
    const state = getState();
    const sessionId = state.activeSessionId;
    const baseWindow = state.windows[state.sessions[sessionId].activeWindowId];
    const roles: WindowRole[] = ["term", "files", "git", "search"];
    const windows = Object.fromEntries(
        roles.map((role) => [
            `window-${role}`,
            {
                ...baseWindow,
                id: `window-${role}`,
                name: role,
                role,
            },
        ]),
    );
    setState({
        sessions: {
            [sessionId]: { ...state.sessions[sessionId], kind: "project", cwd: "/repo", activeWindowId: "window-term" },
        },
        ...withAgents({ windows, windowsBySession: { [sessionId]: roles.map((role) => `window-${role}`) }, agents: {} }, sessionId, [
            { id: "agent-only", type: "codex", title: "agent", startup: "codex", launchState: "live" },
        ]),
    });
});

afterEach(cleanup);

describe("project Git branch chrome", () => {
    it("renders branch status once in the top bar across every project view", () => {
        const sessionId = getState().activeSessionId;
        render(
            <>
                <TopBar />
                <SideRail />
            </>,
        );

        const expectOneTopBarBranch = () => {
            expect(screen.getAllByText(gitStatus.branch)).toHaveLength(1);
            expect(document.querySelector(".tb-git-branch")).toHaveTextContent(gitStatus.branch);
            expect(document.querySelector(".proj-metadata")).not.toBeInTheDocument();
        };

        expectOneTopBarBranch();
        for (const role of ["files", "git", "search", "term"] satisfies WindowRole[]) {
            act(() => {
                const session = getState().sessions[sessionId];
                setState({ sessions: { [sessionId]: { ...session, activeWindowId: `window-${role}` } } });
            });
            expectOneTopBarBranch();
        }

        act(() => {
            const session = getState().sessions[sessionId];
            setState({ sessions: { [sessionId]: { ...session, activeWindowId: agentWindowId(getState(), "agent-only")! } } });
        });
        expectOneTopBarBranch();
    });
});
