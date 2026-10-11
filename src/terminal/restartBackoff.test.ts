import { describe, expect, it } from "vitest";
import { RendererRestartBackoff } from "./restartBackoff";

describe("RendererRestartBackoff", () => {
    it("uses escalating delays for consecutive renderer restarts", () => {
        const backoff = new RendererRestartBackoff({ delaysMs: [10, 20, 40], windowMs: 1_000, maxRestartsPerWindow: 10 });

        expect(backoff.next(0)).toEqual({ delayMs: 10, throttled: false });
        expect(backoff.next(10)).toEqual({ delayMs: 20, throttled: false });
        expect(backoff.next(20)).toEqual({ delayMs: 40, throttled: false });
        expect(backoff.next(30)).toEqual({ delayMs: 40, throttled: false });
    });

    it("enforces a rolling restart budget", () => {
        const backoff = new RendererRestartBackoff({ delaysMs: [10], windowMs: 1_000, maxRestartsPerWindow: 2 });

        expect(backoff.next(0)).toEqual({ delayMs: 10, throttled: false });
        expect(backoff.next(100)).toEqual({ delayMs: 10, throttled: false });
        expect(backoff.next(200)).toEqual({ delayMs: 800, throttled: true });
    });

    it("resets escalation after a stable renderer period", () => {
        const backoff = new RendererRestartBackoff({ delaysMs: [10, 20], windowMs: 1_000, maxRestartsPerWindow: 2 });
        backoff.next(0);
        backoff.next(10);

        backoff.reset();

        expect(backoff.next(20)).toEqual({ delayMs: 10, throttled: false });
    });

    it("forgets an expired failure window", () => {
        const backoff = new RendererRestartBackoff({ delaysMs: [10, 20], windowMs: 100, maxRestartsPerWindow: 1 });
        backoff.next(0);

        expect(backoff.next(101)).toEqual({ delayMs: 10, throttled: false });
    });
});
