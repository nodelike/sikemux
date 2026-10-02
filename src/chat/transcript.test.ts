import { describe, expect, it } from "vitest";
import {
    attachmentName,
    decodedFenceName,
    formatDetail,
    groupParts,
    groupTasks,
    runningSubagents,
    subagentActivity,
    subagentTask,
    taskDetail,
} from "./transcript";
import type { AcpAsyncTask, AcpSubagent, ChatMessage, ChatPart } from "./types";

const text = (id: string): ChatPart => ({ id, kind: "text", text: id });
const tool = (id: string, title = "cargo test", kind?: string): ChatPart => ({ id, kind: "tool", tool: { toolCallId: id, title, kind } });
const subagent = (overrides: Partial<AcpSubagent> = {}): AcpSubagent => ({
    sessionId: "s1",
    name: "explorer",
    task: "Find the bug\nThen explain it",
    state: "running",
    messages: [],
    nextId: 0,
    ...overrides,
});
const message = (id: string, parts: ChatPart[]): ChatMessage => ({ id, role: "assistant", parts });
const task = (overrides: Partial<AcpAsyncTask>): AcpAsyncTask => ({
    asyncTaskId: "t1",
    name: "dev server",
    taskType: "shell",
    description: "",
    state: "running",
    canStop: true,
    ...overrides,
});

describe("formatDetail", () => {
    it("pretty-prints a value", () => {
        expect(formatDetail({ a: 1 })).toBe('{\n  "a": 1\n}');
    });

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
    it("decodes a percent-encoded name", () => {
        expect(decodedFenceName("my%20file.ts")).toBe("my file.ts");
    });

    it("keeps a name that does not decode", () => {
        expect(decodedFenceName("100%.ts")).toBe("100%.ts");
    });
});

describe("groupParts", () => {
    it("gathers a run of tool calls into one group", () => {
        const groups = groupParts([text("a"), tool("b"), tool("c"), text("d"), tool("e")]);
        expect(groups.map((group) => ("tools" in group ? group.tools.map((part) => part.id) : group.id))).toEqual(["a", ["b", "c"], "d", ["e"]]);
    });

    it("names a tool group after its first call", () => {
        expect(groupParts([tool("b"), tool("c")])[0].id).toBe("b");
    });
});

describe("subagentTask", () => {
    it("keeps the first line, trimmed", () => {
        expect(subagentTask("  Find the bug  \nThen explain it")).toBe("Find the bug");
    });
});

describe("subagentActivity", () => {
    it("reports the last tool the subagent called", () => {
        const agent = subagent({
            messages: [message("m1", [tool("a", "/src/one.ts", "read")]), message("m2", [tool("b", "/src/two.ts", "read"), text("c")])],
        });
        expect(subagentActivity(agent)).toBe("read two.ts");
    });

    it("falls back to its task before it has called anything", () => {
        expect(subagentActivity(subagent({ messages: [message("m1", [text("a")])] }))).toBe("Find the bug");
    });
});

describe("runningSubagents", () => {
    it("lists only subagents still at work, in transcript order", () => {
        const first = subagent({ sessionId: "s1" });
        const done = subagent({ sessionId: "s2", state: "completed" });
        const second = subagent({ sessionId: "s3" });
        const messages = [
            message("m1", [{ id: "p1", kind: "subagent", subagent: first }, text("x")]),
            message("m2", [
                { id: "p2", kind: "subagent", subagent: done },
                { id: "p3", kind: "subagent", subagent: second },
            ]),
        ];
        expect(runningSubagents(messages)).toEqual([first, second]);
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
