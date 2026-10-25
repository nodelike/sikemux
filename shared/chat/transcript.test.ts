import { describe, expect, it } from "vitest";
import { attachmentName, decodedFenceName, formatDetail, groupParts, groupTasks, subagentActivity, taskDetail } from "./transcript";
import type { AsyncTask, SubagentCard, ChatPart } from "./types";

const text = (id: string): ChatPart => ({ id, kind: "text", text: id });
const tool = (id: string): ChatPart => ({
    id,
    kind: "tool",
    tool: { toolCallId: id, title: "cargo test", row: { action: "other", target: "cargo test" } },
});
const subagent = (overrides: Partial<SubagentCard> = {}): SubagentCard => ({
    sessionId: "s1",
    name: "explorer",
    task: "Find the bug\nThen explain it",
    state: "running",
    calls: 0,
    ...overrides,
});
const task = (overrides: Partial<AsyncTask>): AsyncTask => ({
    asyncTaskId: "t1",
    name: "dev server",
    taskType: "shell",
    description: "",
    state: "running",
    canStop: true,
    ...overrides,
});

describe("formatDetail", () => {
    it("falls back to String for what JSON cannot write", () => {
        expect(formatDetail(undefined)).toBe("undefined");
        const circular: Record<string, unknown> = {};
        circular.self = circular;
        expect(formatDetail(circular)).toBe("[object Object]");
    });

    it("cuts a huge value short and says so", () => {
        const formatted = formatDetail("x".repeat(130_000));
        expect(formatted.endsWith("\n… output truncated")).toBe(true);
        expect(formatted.length).toBe(120_000 + "\n… output truncated".length);
    });
});

describe("attachmentName", () => {
    it("names a file after its type", () => {
        expect(attachmentName("image/jpeg")).toBe("attachment.jpeg");
        expect(attachmentName("image/svg+xml")).toBe("attachment.svg");
    });

    it("falls back to png when the type says nothing usable", () => {
        expect(attachmentName("")).toBe("attachment.png");
        expect(attachmentName("image/+++")).toBe("attachment.png");
    });
});

describe("decodedFenceName", () => {
    it("keeps a name that does not decode", () => {
        expect(decodedFenceName("100%.ts")).toBe("100%.ts");
    });
});

describe("groupParts", () => {
    it("gathers a run of tool calls into one group", () => {
        const groups = groupParts([text("a"), tool("b"), tool("c"), text("d"), tool("e")]);
        expect(groups.map((group) => ("tools" in group ? group.tools.map((part) => part.id) : group.id))).toEqual(["a", ["b", "c"], "d", ["e"]]);
    });

    it("lets a page an agent showed stand between the calls around it", () => {
        const shown: ChatPart = {
            id: "p",
            kind: "tool",
            tool: { toolCallId: "p", title: "mcp__sikemux-tools__page_show", row: { action: "other", target: "Chart", verb: "show" } },
            page: { id: "0".repeat(32), title: "Chart" },
        };
        const groups = groupParts([tool("a"), shown, tool("b")]);
        expect(groups.map((group) => ("tools" in group ? group.tools.map((part) => part.id) : group.id))).toEqual([["a"], "p", ["b"]]);
    });
});

describe("subagentActivity", () => {
    it("reports the last tool the subagent called", () => {
        const agent = subagent({
            calls: 2,
            lastTool: { toolCallId: "b", title: "/src/two.ts", kind: "read", row: { action: "read", target: "two.ts", path: "/src/two.ts" } },
        });
        expect(subagentActivity(agent)).toBe("read two.ts");
    });

    it("falls back to its task before it has called anything", () => {
        expect(subagentActivity(subagent())).toBe("Find the bug");
    });
});

describe("taskDetail", () => {
    it("takes the first detail that says more than the name", () => {
        expect(taskDetail(task({ summary: "dev server", description: "vite on 5173" }))).toBe("vite on 5173");
        expect(taskDetail(task({ lastToolName: "Bash" }))).toBe("Bash");
    });

    it("says nothing when every detail repeats the name", () => {
        expect(taskDetail(task({ name: "shell", taskType: "shell" }))).toBeUndefined();
    });
});

describe("groupTasks", () => {
    it("groups tasks by type in the order each type first appears", () => {
        const a = task({ asyncTaskId: "a", taskType: "shell" });
        const b = task({ asyncTaskId: "b", taskType: "monitor" });
        const c = task({ asyncTaskId: "c", taskType: "shell" });
        const d = task({ asyncTaskId: "d", taskType: "" });
        expect(groupTasks([a, b, c, d])).toEqual([
            ["shell", [a, c]],
            ["monitor", [b]],
            ["task", [d]],
        ]);
    });
});
