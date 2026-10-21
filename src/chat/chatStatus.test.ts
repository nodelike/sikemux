import { describe, expect, it } from "vitest";
import { effectiveMode as permissionModeOf } from "../agents/agentLaunch";
import { activityText, lostConnectionText } from "./chatStatus";
import type { Agent } from "../state/types";
import type { AcpPermissionRequest, ChatMessage, ChatState } from "./types";

const agent = (overrides: Partial<Agent>) => ({ id: "a1", type: "claude", ...overrides }) as Agent;
const request: AcpPermissionRequest = { requestId: "r", sessionId: "s", toolCall: { toolCallId: "t", title: "Run" }, options: [] };
const message: ChatMessage = { id: "m1", role: "user", parts: [] };

describe("permissionModeOf", () => {
    it("keeps the mode the agent chose", () => {
        expect(permissionModeOf(agent({ permissionMode: "bypass" }))).toBe("bypass");
        expect(permissionModeOf(agent({}))).toBe("workspace-write");
    });
});

describe("activityText", () => {
    const state = (overrides: Partial<ChatState>) =>
        ({ permissions: [], running: false, messages: [], connection: "ready", ...overrides }) as ChatState;

    it("stays quiet while a permission card is showing", () => {
        expect(activityText(state({ permissions: [request], running: true }), "Reading…")).toBeNull();
    });

    it("names the running tool, or thinks without one", () => {
        expect(activityText(state({ running: true }), "Reading…")).toBe("Reading…");
        expect(activityText(state({ running: true }), null)).toBe("Thinking…");
    });

    it("says the agent is reconnecting, with its attempt when known, ahead of the running tool", () => {
        expect(activityText(state({ running: true, retrying: { attempt: 3, connection: true } }), "Reading…")).toBe("Reconnecting… (attempt 3)");
        expect(activityText(state({ running: true, retrying: { attempt: null, connection: true } }), null)).toBe("Reconnecting…");
        expect(activityText(state({ running: true, retrying: { attempt: 2, connection: false } }), null)).toBe("Retrying… (attempt 2)");
    });

    it("shows the connection step under an existing transcript", () => {
        expect(activityText(state({ messages: [message], connection: "starting" }), null)).toBe("Starting agent…");
        expect(activityText(state({ messages: [message] }), null)).toBeNull();
    });

    it("leaves an empty transcript to the connection card", () => {
        expect(activityText(state({ connection: "starting" }), null)).toBeNull();
    });
});

describe("lostConnectionText", () => {
    it("names whose servers the chat lost, or says so generally", () => {
        expect(lostConnectionText("claude")).toMatch(/^Lost connection to Anthropic\./);
        expect(lostConnectionText("codex")).toMatch(/^Lost connection to OpenAI\./);
        expect(lostConnectionText("opencode")).toMatch(/^Lost connection to the model provider\./);
    });
});
