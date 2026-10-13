import { renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useEvery } from "./hooks";

describe("useEvery", () => {
    beforeEach(() => vi.useFakeTimers());
    afterEach(() => vi.useRealTimers());

    it("still fires when the view redraws every second in between", () => {
        const work = vi.fn();
        const { rerender } = renderHook(({ tick }: { tick: number }) => useEvery(true, 5_000, () => work(tick)), {
            initialProps: { tick: 0 },
        });
        for (let tick = 1; tick <= 11; tick++) {
            vi.advanceTimersByTime(1_000);
            rerender({ tick });
        }
        expect(work).toHaveBeenCalledTimes(2);
        expect(work).toHaveBeenLastCalledWith(9);
    });

    it("skips ticks while the window is hidden and reads once when it is shown", () => {
        const work = vi.fn();
        renderHook(() => useEvery(true, 5_000, work));
        Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
        vi.advanceTimersByTime(20_000);
        expect(work).not.toHaveBeenCalled();
        Object.defineProperty(document, "hidden", { configurable: true, get: () => false });
        document.dispatchEvent(new Event("visibilitychange"));
        expect(work).toHaveBeenCalledTimes(1);
    });

    it("does nothing while nothing is live", () => {
        const work = vi.fn();
        renderHook(() => useEvery(false, 5_000, work));
        vi.advanceTimersByTime(20_000);
        expect(work).not.toHaveBeenCalled();
    });
});
