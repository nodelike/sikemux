import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { performanceTelemetry } from "../lib/performance";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests } from "./transport";
import {
    applyPtyShellMetadataEvent,
    parsePtyShellMetadataEvent,
    PTY_SHELL_METADATA_EVENT,
    resetPtyShellSubscriptionsForTests,
    subscribePtyShellMetadata,
} from "./ptyShell";

let transport: MemoryIpcTransport;

beforeEach(() => {
    resetIpcTransportForTests();
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
});

afterEach(async () => {
    await resetPtyShellSubscriptionsForTests();
    resetIpcTransportForTests();
    performanceTelemetry.reset();
});

const event = (overrides: Record<string, unknown> = {}) => ({
    ptyId: 42,
    revision: 1,
    boundary: "prompt_start",
    cwd: "/repo",
    phase: "prompt",
    ...overrides,
});

describe("PTY shell metadata adapter", () => {
    it("parses display-only events", () => {
        expect(parsePtyShellMetadataEvent(event())).toEqual({
            ptyId: 42,
            revision: 1,
            boundary: "prompt_start",
            cwd: "/repo",
            phase: "prompt",
            exitCode: null,
        });
        expect(parsePtyShellMetadataEvent(event({ boundary: "command_finished", phase: "finished", exitCode: 7 }))).toMatchObject({
            exitCode: 7,
        });
    });

    it("merges monotonic events and preserves the last exit status", () => {
        const prompt = parsePtyShellMetadataEvent(event())!;
        const first = applyPtyShellMetadataEvent(null, prompt);
        const finished = parsePtyShellMetadataEvent(event({ revision: 2, boundary: "command_finished", phase: "finished", exitCode: 3 }))!;
        const second = applyPtyShellMetadataEvent(first, finished);
        const running = parsePtyShellMetadataEvent(event({ revision: 3, boundary: "command_start", phase: "running" }))!;

        expect(second.lastExitCode).toBe(3);
        expect(applyPtyShellMetadataEvent(second, running).lastExitCode).toBe(3);
        expect(applyPtyShellMetadataEvent(second, prompt)).toBe(second);
    });

    it("shares one transport listener, routes by PTY, and tears down at zero subscribers", async () => {
        const first = vi.fn();
        const second = vi.fn();
        const unsubscribeFirst = await subscribePtyShellMetadata(42, first);
        const unsubscribeSecond = await subscribePtyShellMetadata(7, second);

        expect(transport.eventListenerCount).toBe(1);
        expect(transport.emit(PTY_SHELL_METADATA_EVENT, event())).toBe(1);
        expect(first).toHaveBeenCalledOnce();
        expect(second).not.toHaveBeenCalled();

        unsubscribeFirst();
        expect(transport.eventListenerCount).toBe(1);
        unsubscribeSecond();
        expect(transport.eventListenerCount).toBe(0);
        unsubscribeSecond();
        expect(transport.eventListenerCount).toBe(0);
    });

    it("drops invalid payloads", async () => {
        const listener = vi.fn();
        const unsubscribe = await subscribePtyShellMetadata(42, listener);
        transport.emit(PTY_SHELL_METADATA_EVENT, event({ phase: "forged" }));
        transport.emit(PTY_SHELL_METADATA_EVENT, event());

        expect(listener).toHaveBeenCalledOnce();
        expect(performanceTelemetry.snapshot().counters).toMatchObject({ "terminal.shell-metadata.invalid-events": 1 });
        unsubscribe();
    });
});
