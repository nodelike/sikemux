import { describe, expect, it } from "vitest";
import type { AcpEvent } from "../api/acp";
import { eventMessage, permissionRequest, promptAction, recordOf, statusFromEvent } from "./acpEvents";

const event = (kind: AcpEvent["kind"], payload: Record<string, unknown>): AcpEvent => ({ agentId: "a1", kind, payload });

describe("recordOf", () => {
    it("takes a plain object as it is", () => {
        const value = { a: 1 };
        expect(recordOf(value)).toBe(value);
    });

    it("refuses arrays, null and primitives", () => {
        expect(recordOf([1, 2])).toBeNull();
        expect(recordOf(null)).toBeNull();
        expect(recordOf("text")).toBeNull();
        expect(recordOf(3)).toBeNull();
        expect(recordOf(undefined)).toBeNull();
    });
});

describe("eventMessage", () => {
    it("reads the message the adapter sent", () => {
        expect(eventMessage(event("error", { message: "rate limited" }))).toBe("rate limited");
    });

    it("falls back when the message is missing or not text", () => {
        expect(eventMessage(event("error", {}))).toBe("ACP session failed");
        expect(eventMessage(event("error", { message: 42 }))).toBe("ACP session failed");
    });
});

describe("statusFromEvent", () => {
    it("passes every known state through", () => {
        for (const state of ["installing", "starting", "initializing", "ready", "stopped", "error"] as const)
            expect(statusFromEvent(event("status", { state }))).toBe(state);
    });

    it("reads anything else as still connecting", () => {
        expect(statusFromEvent(event("status", { state: "warming" }))).toBe("connecting");
        expect(statusFromEvent(event("status", {}))).toBe("connecting");
    });
});

describe("permissionRequest", () => {
    const valid = {
        requestId: "r1",
        sessionId: "s1",
        toolCall: { toolCallId: "t1", title: "Run cargo test", kind: "execute" },
        options: [
            { optionId: "allow", name: "Allow", kind: "allow_once" },
            { optionId: "reject", name: "Reject", kind: "reject_once" },
        ],
    };

    it("reads a well-formed request", () => {
        expect(permissionRequest(valid)).toEqual({
            requestId: "r1",
            sessionId: "s1",
            toolCall: { toolCallId: "t1", title: "Run cargo test", kind: "execute" },
            options: [
                { optionId: "allow", name: "Allow", kind: "allow_once" },
                { optionId: "reject", name: "Reject", kind: "reject_once" },
            ],
        });
    });

    it("names an untitled tool call", () => {
        expect(permissionRequest({ ...valid, toolCall: { toolCallId: "t1" } })?.toolCall.title).toBe("Agent tool");
    });

    it("drops options that are missing a field", () => {
        const request = permissionRequest({
            ...valid,
            options: [{ optionId: "allow", name: "Allow", kind: "allow_once" }, { optionId: "half", name: "Half" }, "junk", null],
        });
        expect(request?.options).toEqual([{ optionId: "allow", name: "Allow", kind: "allow_once" }]);
    });

    it("refuses a request missing what it needs to be answered", () => {
        expect(permissionRequest({ ...valid, requestId: undefined })).toBeNull();
        expect(permissionRequest({ ...valid, sessionId: 7 })).toBeNull();
        expect(permissionRequest({ ...valid, toolCall: null })).toBeNull();
        expect(permissionRequest({ ...valid, toolCall: { title: "no id" } })).toBeNull();
        expect(permissionRequest({ ...valid, options: "allow" })).toBeNull();
    });
});

describe("promptAction", () => {
    it("shows a prompt from another device the way a typed one shows", () => {
        expect(promptAction({ text: "fix the test", paths: ["/tmp/a.ts", 3] })).toEqual({
            type: "local_prompt",
            text: "fix the test",
            paths: ["/tmp/a.ts"],
        });
    });

    it("ignores a prompt with no text", () => {
        expect(promptAction({ paths: [] })).toBeNull();
    });
});
