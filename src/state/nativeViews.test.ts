import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
    nativeViewsOccluded,
    occludeNativeViews,
    onStageFrame,
    setNativeViewHoles,
    stageMoving,
    useNativeViewHoles,
    useNativeViewsOccluded,
    useOccludeNativeViews,
    useStageMotion,
    useStageMoving,
    whenStageStill,
} from "./nativeViews";

beforeEach(() => {
    vi.useFakeTimers({ toFake: ["requestAnimationFrame", "cancelAnimationFrame"] });
});

const TOASTS = {};
const MENU = {};

afterEach(() => {
    setNativeViewHoles(TOASTS, []);
    setNativeViewHoles(MENU, []);
    vi.useRealTimers();
});

describe("occluding native views", () => {
    it("keeps the pages aside until every overlay that asked has let go, in any order", () => {
        const { result } = renderHook(() => useNativeViewsOccluded());
        expect(result.current).toBe(false);

        let first = () => {};
        let second = () => {};
        act(() => {
            first = occludeNativeViews();
            second = occludeNativeViews();
        });
        expect(result.current).toBe(true);

        act(() => first());
        expect(result.current).toBe(true);
        act(() => first());
        expect(nativeViewsOccluded()).toBe(true);

        act(() => second());
        expect(result.current).toBe(false);
    });

    it("holds them aside only while the overlay is active", () => {
        const { rerender, unmount } = renderHook(({ active }) => useOccludeNativeViews(active), { initialProps: { active: false } });
        expect(nativeViewsOccluded()).toBe(false);

        rerender({ active: true });
        expect(nativeViewsOccluded()).toBe(true);

        rerender({ active: false });
        expect(nativeViewsOccluded()).toBe(false);

        rerender({ active: true });
        unmount();
        expect(nativeViewsOccluded()).toBe(false);
    });
});

describe("holes in native views", () => {
    const toast = { x: 10, y: 20, width: 200, height: 40, radius: 8 };

    it("tells its readers about new holes but not about the same holes again", () => {
        const renders = vi.fn();
        const { result } = renderHook(() => {
            renders();
            return useNativeViewHoles();
        });
        expect(result.current).toEqual([]);

        act(() => setNativeViewHoles(TOASTS, [toast]));
        expect(result.current).toEqual([toast]);
        const count = renders.mock.calls.length;

        act(() => setNativeViewHoles(TOASTS, [{ ...toast }]));
        expect(renders).toHaveBeenCalledTimes(count);

        act(() => setNativeViewHoles(TOASTS, [{ ...toast, radius: 4 }]));
        expect(result.current).toEqual([{ ...toast, radius: 4 }]);

        act(() => setNativeViewHoles(TOASTS, []));
        expect(result.current).toEqual([]);
    });
});

describe("holes from more than one overlay", () => {
    const toast = { x: 10, y: 20, width: 200, height: 40, radius: 8 };
    const menu = { x: 300, y: 60, width: 196, height: 240, radius: 10 };

    it("cuts every overlay's holes and withdraws only the one that lets go", () => {
        const { result } = renderHook(() => useNativeViewHoles());

        act(() => setNativeViewHoles(TOASTS, [toast]));
        act(() => setNativeViewHoles(MENU, [menu]));
        expect(result.current).toEqual([toast, menu]);

        act(() => setNativeViewHoles(MENU, []));
        expect(result.current).toEqual([toast]);
    });
});

describe("stage motion", () => {
    it("follows every frame only while the stage travels", () => {
        const follow = vi.fn();
        const stopFollowing = onStageFrame(follow);
        const moving = renderHook(() => useStageMoving());
        const { rerender, unmount } = renderHook(({ active }) => useStageMotion(active), { initialProps: { active: true } });

        expect(moving.result.current).toBe(true);
        vi.advanceTimersToNextFrame();
        vi.advanceTimersToNextFrame();
        expect(follow).toHaveBeenCalledTimes(2);

        rerender({ active: false });
        expect(moving.result.current).toBe(false);
        vi.advanceTimersToNextFrame();
        expect(follow).toHaveBeenCalledTimes(2);

        unmount();
        stopFollowing();
    });

    it("runs a still callback on the next frame when the stage is not travelling", () => {
        const still = vi.fn();
        whenStageStill(still);
        expect(still).not.toHaveBeenCalled();

        vi.advanceTimersToNextFrame();
        expect(still).toHaveBeenCalledOnce();
    });

    it("waits for a slide to finish before running a still callback", () => {
        const still = vi.fn();
        const { rerender, unmount } = renderHook(({ active }) => useStageMotion(active), { initialProps: { active: true } });
        whenStageStill(still);
        vi.advanceTimersToNextFrame();
        expect(still).not.toHaveBeenCalled();

        rerender({ active: false });
        expect(stageMoving()).toBe(false);
        expect(still).toHaveBeenCalledOnce();

        rerender({ active: true });
        rerender({ active: false });
        expect(still).toHaveBeenCalledOnce();
        unmount();
    });

    it("drops a still callback that is cancelled before or during a slide", () => {
        const early = vi.fn();
        whenStageStill(early)();
        vi.advanceTimersToNextFrame();
        expect(early).not.toHaveBeenCalled();

        const late = vi.fn();
        const { rerender, unmount } = renderHook(({ active }) => useStageMotion(active), { initialProps: { active: true } });
        const cancel = whenStageStill(late);
        vi.advanceTimersToNextFrame();
        cancel();
        rerender({ active: false });
        expect(late).not.toHaveBeenCalled();
        unmount();
    });
});
