import { describe, expect, it } from "vitest";
import { activityText, backendState, composerPlaceholder, connectingLabel, knownEffort, permissionModeOf } from "./chatStatus";
import type { Agent } from "../state/types";
import type { AcpPermissionRequest, ChatMessage, ChatState } from "./types";

const agent = (overrides: Partial<Agent>) => ({ id: "a1", type: "claude", ...overrides }) as Agent;
const request: AcpPermissionRequest = { requestId: "r", sessionId: "s", toolCall: { toolCallId: "t", title: "Run" }, options: [] };
const message: ChatMessage = { id: "m1", role: "user", parts: [] };

describe("permissionModeOf", () => {
    it("keeps the mode the agent chose", () => {
        expect(permissionModeOf(agent({ permissionMode: "read-only", skipPermissions: true }))).toBe("read-only");
    });

    it("reads the old skip flag when no mode was chosen", () => {
        expect(permissionModeOf(agent({ skipPermissions: true }))).toBe("bypass");
        expect(permissionModeOf(agent({}))).toBe("workspace-write");
    });
});

describe("knownEffort", () => {
    it("keeps an effort level the app knows", () => {
        expect(knownEffort("high")).toBe("high");
        expect(knownEffort("off")).toBe("off");
    });

    it("drops one it does not", () => {
        expect(knownEffort("turbo")).toBeUndefined();
        expect(knownEffort(undefined)).toBeUndefined();
    });
});

describe("backendState", () => {
    const at = (connection: ChatState["connection"], awaitingPermission = false, running = false) =>
        backendState({ connection, awaitingPermission, running });

    it("reports a dropped session as stopped whatever else is going on", () => {
        expect(at("error", true, true)).toBe("stopped");
        expect(at("stopped")).toBe("stopped");
    });

    it("reports a permission ask before a running turn", () => {
        expect(at("ready", true, true)).toBe("blocked");
        expect(at("ready", false, true)).toBe("working");
    });

    it("is idle when ready and unknown while still connecting", () => {
        expect(at("ready")).toBe("idle");
        expect(at("connecting")).toBe("unknown");
    });
});

describe("connectingLabel", () => {
    it("describes each step of coming up", () => {
        expect(connectingLabel("installing")).toBe("Installing structured-session adapter…");
        expect(connectingLabel("starting")).toBe("Starting agent adapter…");
        expect(connectingLabel("connecting")).toBe("Connecting to agent session…");
        expect(connectingLabel("initializing")).toBe("Connecting to agent session…");
    });

    it("says nothing once the session is up or down", () => {
        expect(connectingLabel("ready")).toBeNull();
        expect(connectingLabel("error")).toBeNull();
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

    it("shows the connection step under an existing transcript", () => {
        expect(activityText(state({ messages: [message], connection: "starting" }), null)).toBe("Starting agent adapter…");
        expect(activityText(state({ messages: [message] }), null)).toBeNull();
    });

    it("leaves an empty transcript to the connection card", () => {
        expect(activityText(state({ connection: "starting" }), null)).toBeNull();
    });
});

describe("composerPlaceholder", () => {
    const quiet = { resuming: false, disconnected: false };

    it("invites a prompt, or a queued one mid-turn, when ready", () => {
        expect(composerPlaceholder({ connection: "ready", running: false }, quiet)).toBe("Ask about this project, or type / for commands");
        expect(composerPlaceholder({ connection: "ready", running: true }, quiet)).toBe("Send to queue behind the running turn");
    });

    it("explains a dropped session", () => {
        expect(composerPlaceholder({ connection: "error", running: false }, { resuming: true, disconnected: true })).toBe(
            "Resuming — this message sends as soon as the session is back",
        );
        expect(composerPlaceholder({ connection: "stopped", running: false }, { resuming: false, disconnected: true })).toBe(
            "Reconnect to continue this conversation",
        );
    });

    it("describes each step of coming up", () => {
        expect(composerPlaceholder({ connection: "installing", running: false }, quiet)).toBe("Installing structured-session adapter…");
        expect(composerPlaceholder({ connection: "starting", running: false }, quiet)).toBe("Starting agent adapter…");
        expect(composerPlaceholder({ connection: "initializing", running: false }, quiet)).toBe("Connecting to agent session…");
    });
});
