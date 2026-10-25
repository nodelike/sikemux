import { expect, it } from "vitest";
import { durationLabel, elapsedLabel } from "./durationLabel";

it("keeps a quick tool call legible instead of rounding it away", () => {
    expect(durationLabel(0)).toBe("0ms");
    expect(durationLabel(3)).toBe("3ms");
    expect(durationLabel(47)).toBe("47ms");
    expect(durationLabel(999)).toBe("999ms");
});

it("switches to seconds once there are seconds to show", () => {
    expect(durationLabel(1000)).toBe("1s");
    expect(durationLabel(1400)).toBe("1s");
    expect(durationLabel(59_000)).toBe("59s");
});

it("spells a long call out in minutes", () => {
    expect(durationLabel(60_000)).toBe("1m 00s");
    expect(durationLabel(125_000)).toBe("2m 05s");
});

it("never reports a clock that went backwards as a negative", () => {
    expect(durationLabel(-5)).toBe("0ms");
});

it("counts whole seconds on the live clock", () => {
    expect(elapsedLabel(0)).toBe("0s");
    expect(elapsedLabel(59)).toBe("59s");
});

it("spells a long wait out in minutes on the live clock", () => {
    expect(elapsedLabel(60)).toBe("1m 00s");
    expect(elapsedLabel(605)).toBe("10m 05s");
});
