import { describe, expect, it } from "vitest";
import { billedMinutes } from "./RunUsage";

describe("billedMinutes", () => {
    it("bills each runner by the started minute, the way GitHub does", () => {
        expect(billedMinutes([{ runner: "UBUNTU", totalMs: 61_000, jobs: 1 }])).toBe(2);
        expect(billedMinutes([{ runner: "UBUNTU", totalMs: 60_000, jobs: 1 }])).toBe(1);
    });

    it("adds the runners up", () => {
        expect(
            billedMinutes([
                { runner: "UBUNTU", totalMs: 120_000, jobs: 2 },
                { runner: "MACOS", totalMs: 180_000, jobs: 1 },
            ]),
        ).toBe(5);
    });

    it("bills nothing for a public repository, which reports nothing", () => {
        expect(billedMinutes([])).toBe(0);
    });
});
