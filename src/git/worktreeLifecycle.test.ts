import { describe, expect, it } from "vitest";
import { worktreeHasLiveOwners } from "./worktreeLifecycle";

describe("worktree lifecycle", () => {
    it("protects project sessions and agents currently using a checkout", () => {
        const project = {
            id: "project",
            name: "lane",
            kind: "project" as const,
            cwd: "/work/lane",
            pinned: false,
            activeWindowId: "window",
        };
        expect(worktreeHasLiveOwners({ sessions: { project }, agents: {} }, "/work/lane")).toBe(true);
        expect(
            worktreeHasLiveOwners(
                {
                    sessions: {},
                    agents: { agent: { id: "agent", type: "codex", title: "agent", startup: "codex", cwd: "/work/lane" } },
                },
                "/work/lane",
            ),
        ).toBe(true);
        expect(worktreeHasLiveOwners({ sessions: {}, agents: {} }, "/work/lane")).toBe(false);
    });

    it("counts an agent working in a folder inside the checkout, but not the agent asking", () => {
        const agents = { agent: { id: "agent", type: "codex" as const, title: "agent", startup: "codex", cwd: "/work/lane/packages/app" } };
        expect(worktreeHasLiveOwners({ sessions: {}, agents }, "/work/lane")).toBe(true);
        expect(worktreeHasLiveOwners({ sessions: {}, agents }, "/work/lane", "agent")).toBe(false);
        expect(worktreeHasLiveOwners({ sessions: {}, agents }, "/work/lane-2")).toBe(false);
    });
});
