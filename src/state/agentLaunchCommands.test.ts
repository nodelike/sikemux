import { beforeEach, describe, expect, it } from "vitest";
import { addAgent, closeAgentPalette, focusAgents, setAgentPermissionMode } from "./commands";
import { getState, setState } from "./store";
import { activeAgentId, agentIdsOf } from "./selectors";

const initial = getState();

beforeEach(() => {
    setState(initial, true);
    const session = {
        id: "project",
        name: "project",
        kind: "project" as const,
        cwd: "/code/project",
        deploy: null,
        pinned: false,
        activeWindowId: "window",
    };
    setState({
        sessions: { project: session },
        sessionOrder: ["project"],
        activeSessionId: "project",
        agents: {},
    });
});

describe("agent launch commands", () => {
    it("opens and pins the agent picker when an empty project has nothing else to show", () => {
        focusAgents();

        expect(getState().agentPaletteOpen).toBe(true);

        closeAgentPalette();
        expect(getState().agentPaletteOpen).toBe(true);

        const pane = { type: "pane" as const, id: "window-pane", cwd: "/code/project", kind: "terminal" as const, title: "1" };
        setState((state) => ({
            windows: { ...state.windows, window: { id: "window", name: "1", role: "term" as const, root: pane, activePaneId: pane.id } },
            windowsBySession: { ...state.windowsBySession, project: ["window"] },
        }));
        closeAgentPalette();
        expect(getState().agentPaletteOpen).toBe(false);
    });

    it("focuses an existing agent without opening the picker", () => {
        addAgent("codex");
        setState((state) => ({
            sessions: { ...state.sessions, project: { ...state.sessions.project, activeWindowId: "window" } },
        }));

        focusAgents();

        expect(activeAgentId(getState(), getState().sessions.project)).toBe(agentIdsOf(getState(), "project")[0]);
        expect(getState().agentPaletteOpen).toBe(false);
    });

    it("does not attach a mismatched provider profile to a launch", () => {
        setState({ providerProfiles: [{ id: "codex-only", name: "Codex", provider: "codex", accent: "#abcdef", executablePath: "/bin/codex" }] });

        addAgent("claude", undefined, undefined, { profileId: "codex-only", permissionMode: "workspace-write" });

        const agent = getState().agents[agentIdsOf(getState(), "project")[0]];
        expect(agent.profileId).toBeUndefined();
    });

    it("pins an asynchronous launch to its originating project", () => {
        const other = { ...getState().sessions.project, id: "other", cwd: "/code/other", name: "other" };
        setState((state) => ({
            sessions: { ...state.sessions, other },
            sessionOrder: ["project", "other"],
            activeSessionId: "other",
        }));

        const attached = addAgent("codex", undefined, undefined, { sessionId: "project", cwd: "/code/project-lane" });

        expect(attached).toBe(true);
        expect(agentIdsOf(getState(), "project")).toHaveLength(1);
        expect(agentIdsOf(getState(), "other")).toHaveLength(0);
        expect(getState().activeSessionId).toBe("other");
    });

    it("keeps model and effort overrides on a new agent", () => {
        addAgent("codex", undefined, "Repair parser", {
            model: "gpt-5.6-codex",
            effort: "high",
        });

        const agent = getState().agents[agentIdsOf(getState(), "project")[0]];
        expect(agent).toMatchObject({
            model: "gpt-5.6-codex",
            effort: "high",
            cwd: "/code/project",
        });
        expect(agent).not.toHaveProperty("initialInput");
        expect(agent).not.toHaveProperty("worktreePath");
    });

    it("relaunches a resumable session in Normal or YOLO mode", () => {
        addAgent("codex", "session-42", "Resumable task", { permissionMode: "workspace-write" });
        const id = agentIdsOf(getState(), "project")[0];

        setAgentPermissionMode(id, "bypass");
        expect(getState().agents[id]).toMatchObject({ permissionMode: "bypass" });

        setAgentPermissionMode(id, "workspace-write");
        expect(getState().agents[id]).toMatchObject({ permissionMode: "workspace-write" });
    });

    it("relaunches a fresh CLI when its live PTY mode changes", () => {
        addAgent("claude", undefined, "Fresh task", { permissionMode: "workspace-write" });
        const id = agentIdsOf(getState(), "project")[0];

        setAgentPermissionMode(id, "bypass");

        expect(getState().agents[id]).toMatchObject({
            permissionMode: "bypass",
        });
    });
});
