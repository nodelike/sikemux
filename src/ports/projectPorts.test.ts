import { beforeEach, describe, expect, it } from "vitest";
import { getState, setState } from "../state/store";
import { agentPort, seedProjects, terminalPort } from "../test/ports";
import { deskAgentFor, hasLiveWork, previewPort, projectPorts } from "./projectPorts";

const initial = getState();

beforeEach(() => {
    setState(initial, true);
});

describe("projectPorts", () => {
    it("keeps the ports of this project's terminals and agents and drops the rest", () => {
        const state = seedProjects();
        const ports = projectPorts(state, "project", [
            terminalPort(5173, { paneId: "pane-1", project: "/code" }),
            agentPort(3000, "agent-1"),
            agentPort(4000, "agent-9"),
            terminalPort(6000, { project: "/other" }),
            terminalPort(7000, { agentId: "agent-9", project: "/code" }),
        ]);
        expect(ports.map((port) => [port.port, port.owner.kind, port.owner.label])).toEqual([
            [3000, "agent", "Claude"],
            [5173, "terminal", "npm run dev"],
        ]);
        expect(ports[1].owner.reveal).toEqual({ kind: "pane", sessionId: "project", windowId: "win-shell", paneId: "pane-1" });
        expect(ports[0].url).toBe("http://localhost:3000/");
    });

    it("puts the preview port first and opens the preview URL itself", () => {
        const state = seedProjects();
        const ports = projectPorts(
            state,
            "project",
            [terminalPort(3000, { project: "/code" }), terminalPort(8080, { project: "/code", taskExecutionId: "run-1" })],
            "http://localhost:8080/app",
        );
        expect(ports.map((port) => [port.port, port.preview])).toEqual([
            [8080, true],
            [3000, false],
        ]);
        expect(ports[0].url).toBe("http://localhost:8080/app");
        expect(ports[0].owner).toEqual({ kind: "task", label: "task", reveal: null });
    });

    it("reads the port out of a local preview URL only", () => {
        expect(previewPort("http://127.0.0.1:4321")).toBe(4321);
        expect(previewPort("http://localhost/")).toBe(80);
        expect(previewPort("https://example.com:8443")).toBeNull();
        expect(previewPort("not a url")).toBeNull();
        expect(previewPort(undefined)).toBeNull();
    });
});

describe("which agent's desk a port opens on", () => {
    it("is the agent in front when it is running", () => {
        expect(deskAgentFor(seedProjects({ front: "agent-1" }), "project")).toBe("agent-1");
    });

    it("is otherwise the running agent that worked last", () => {
        seedProjects();
        setState({ agentActivity: { "agent-1": { updatedAt: 1 }, "agent-2": { updatedAt: 5 } } } as never);
        expect(deskAgentFor(getState(), "project")).toBe("agent-2");
    });

    it("is nobody when every agent is asleep, so nothing gets woken", () => {
        expect(deskAgentFor(seedProjects({ agentLaunch: "dormant", front: "agent-1" }), "project")).toBeNull();
    });
});

describe("hasLiveWork", () => {
    it("is true with a terminal or a running agent", () => {
        expect(hasLiveWork(seedProjects({ agentLaunch: "dormant" }), "project")).toBe(true);
        expect(hasLiveWork(seedProjects({ terminal: false }), "project")).toBe(true);
    });

    it("is false with neither", () => {
        expect(hasLiveWork(seedProjects({ terminal: false, agentLaunch: "dormant" }), "project")).toBe(false);
    });
});
