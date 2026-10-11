import { describe, expect, it, vi } from "vitest";
import { UI_ACTIVITY_LIMITS, UiActivityTracker, UNTRACKED_COMMAND, type UiActivityReport } from "./activity";

function trackerAt(clock: { now: number }, options: { visible?: () => boolean } = {}) {
    return new UiActivityTracker({
        now: () => clock.now,
        wallClock: () => 1_700_000_000_000 + clock.now,
        visible: options.visible ?? (() => true),
    });
}

describe("UiActivityTracker in-flight commands", () => {
    it("ages outstanding commands and forgets them once they settle", () => {
        const clock = { now: 0 };
        const tracker = trackerAt(clock);
        const slow = tracker.beginCommand("git_status");
        clock.now = 40;
        const fast = tracker.beginCommand("pty_write");
        clock.now = 100;

        expect(tracker.snapshot().inflight).toEqual([
            { command: "git_status", ageMs: 100 },
            { command: "pty_write", ageMs: 60 },
        ]);

        tracker.endCommand(fast, true);
        expect(tracker.inflightCount).toBe(1);
        tracker.endCommand(slow, false);
        expect(tracker.inflightCount).toBe(0);
        expect(tracker.snapshot()).toMatchObject({
            inflight: [],
            recent: [
                { command: "git_status", ms: 100, ok: false },
                { command: "pty_write", ms: 60, ok: true },
            ],
        });
    });

    it("ignores an untracked ticket and a ticket that already settled", () => {
        const clock = { now: 0 };
        const tracker = trackerAt(clock);
        const ticket = tracker.beginCommand("boot_init");
        tracker.endCommand(ticket, true);
        tracker.endCommand(ticket, true);
        tracker.endCommand(UNTRACKED_COMMAND, false);
        expect(tracker.snapshot().recent).toHaveLength(1);
    });

    it("stops naming commands past the in-flight ceiling instead of growing", () => {
        const clock = { now: 0 };
        const tracker = new UiActivityTracker({ now: () => clock.now, maxInflight: 4 });
        const tickets = Array.from({ length: 10 }, (_, index) => tracker.beginCommand(`cmd_${index}`));

        expect(tracker.inflightCount).toBe(4);
        expect(tracker.droppedInflightCount).toBe(6);
        expect(tickets.slice(4).every((ticket) => ticket === UNTRACKED_COMMAND)).toBe(true);

        for (const ticket of tickets) tracker.endCommand(ticket, true);
        expect(tracker.inflightCount).toBe(0);
        expect(tracker.snapshot().recent).toHaveLength(4);
    });
});

describe("UiActivityTracker bounds", () => {
    it("keeps only the newest entries under a flood", () => {
        const clock = { now: 0 };
        const tracker = trackerAt(clock);
        for (let index = 0; index < 5_000; index += 1) {
            const ticket = tracker.beginCommand(`cmd_${index}`);
            clock.now += 1;
            tracker.endCommand(ticket, index % 2 === 0);
            tracker.recordInteraction(index % 2 === 0 ? "keyboard" : "pointer");
        }

        const report = tracker.snapshot();
        expect(report.recent).toHaveLength(UI_ACTIVITY_LIMITS.maxEntries);
        expect(report.interactions).toHaveLength(UI_ACTIVITY_LIMITS.maxEntries);
        expect(report.recent[0].command).toBe("cmd_4999");
        expect(report.recent.at(-1)?.command).toBe(`cmd_${5_000 - UI_ACTIVITY_LIMITS.maxEntries}`);
        expect(report.recent.some((entry) => entry.command === "cmd_0")).toBe(false);
        expect(report.interactions[0].ageMs).toBe(0);
        expect(report.interactions.at(-1)?.ageMs).toBe(UI_ACTIVITY_LIMITS.maxEntries - 1);
    });

    it("caps in-flight and rejection lists at the report limit", () => {
        const clock = { now: 0 };
        const tracker = new UiActivityTracker({ now: () => clock.now, maxInflight: 200 });
        tracker.setSources({
            rejections: () => Array.from({ length: 500 }, (_, index) => ({ message: `boom ${index}`, count: 500 - index })),
        });
        for (let index = 0; index < 200; index += 1) tracker.beginCommand(`cmd_${index}`);

        const report = tracker.snapshot();
        expect(report.inflight).toHaveLength(UI_ACTIVITY_LIMITS.maxEntries);
        expect(report.rejections).toHaveLength(UI_ACTIVITY_LIMITS.maxEntries);
        expect(report.inflight[0].command).toBe("cmd_0");
    });

    it("truncates every string it passes on", () => {
        const long = "x".repeat(4_000);
        const tracker = new UiActivityTracker({ now: () => 0 });
        tracker.setSources({ focusPane: () => long, rejections: () => [{ message: long, count: 2 }] });
        tracker.beginCommand(long);
        tracker.recordInteraction(long);
        const ticket = tracker.beginCommand(long);
        tracker.endCommand(ticket, true);

        const report = tracker.snapshot();
        const limit = UI_ACTIVITY_LIMITS.maxStringLength;
        expect(report.focusPane).toHaveLength(limit);
        expect(report.inflight[0].command).toHaveLength(limit);
        expect(report.recent[0].command).toHaveLength(limit);
        expect(report.interactions[0].kind).toHaveLength(limit);
        expect(report.rejections[0].message).toHaveLength(limit);
    });
});

describe("UiActivityTracker report", () => {
    it("matches the native contract once serialized", () => {
        const clock = { now: 0 };
        const tracker = trackerAt(clock);
        tracker.setSources({ focusPane: () => "terminal", rejections: () => [{ message: "undefined is not an object", count: 3 }] });
        tracker.beginCommand("git_status");
        clock.now = 25;
        tracker.endCommand(tracker.beginCommand("home_dir"), true);
        tracker.recordInteraction("keyboard");

        const report = JSON.parse(JSON.stringify(tracker.snapshot())) as UiActivityReport;
        expect(Object.keys(report)).toEqual(["atMs", "inflight", "recent", "focusPane", "interactions", "rejections"]);
        expect(report.atMs).toBe(1_700_000_000_025);
        expect(Object.keys(report.inflight[0])).toEqual(["command", "ageMs"]);
        expect(Object.keys(report.recent[0])).toEqual(["command", "ms", "ok"]);
        expect(Object.keys(report.interactions[0])).toEqual(["kind", "ageMs"]);
        expect(Object.keys(report.rejections[0])).toEqual(["message", "count"]);
        expect(report).toEqual({
            atMs: 1_700_000_000_025,
            inflight: [{ command: "git_status", ageMs: 25 }],
            recent: [{ command: "home_dir", ms: 0, ok: true }],
            focusPane: "terminal",
            interactions: [{ kind: "keyboard", ageMs: 0 }],
            rejections: [{ message: "undefined is not an object", count: 3 }],
        });
    });

    it("reports whole milliseconds, which is all the native side accepts", () => {
        const clock = { now: 0.5 };
        const tracker = trackerAt(clock);
        const ticket = tracker.beginCommand("git_status");
        tracker.beginCommand("home_dir");
        tracker.recordInteraction("pointer");
        clock.now = 12.74;
        tracker.endCommand(ticket, true);

        const report = tracker.snapshot();
        expect(report.recent[0].ms).toBe(12);
        expect(report.inflight[0].ageMs).toBe(12);
        expect(report.interactions[0].ageMs).toBe(12);
        expect(report.atMs).toBe(1_700_000_000_013);
    });

    it("survives a source that throws", () => {
        const tracker = new UiActivityTracker({ now: () => 0 });
        tracker.setSources({
            focusPane: () => {
                throw new Error("store is gone");
            },
            rejections: () => {
                throw new Error("counts are gone");
            },
        });
        expect(tracker.snapshot()).toMatchObject({ focusPane: null, rejections: [] });
    });

    it("does no work while the window is hidden", () => {
        const clock = { now: 0 };
        let visible = false;
        const focusPane = vi.fn(() => "editor");
        const rejections = vi.fn(() => []);
        const tracker = trackerAt(clock, { visible: () => visible });
        tracker.setSources({ focusPane, rejections });
        tracker.beginCommand("git_status");

        expect(tracker.report()).toBeNull();
        expect(focusPane).not.toHaveBeenCalled();
        expect(rejections).not.toHaveBeenCalled();

        visible = true;
        expect(tracker.report()?.focusPane).toBe("editor");
        expect(focusPane).toHaveBeenCalledTimes(1);
    });
});
