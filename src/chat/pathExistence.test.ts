import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ pathKinds: vi.fn() }));

vi.mock("../api/fs", () => ({ fsapi: { pathKinds: mocks.pathKinds } }));

const { forgetPathState, usePathState } = await import("./pathExistence");

const FILES = new Set(["/repo/a.ts", "/repo/b.ts"]);

beforeEach(() => {
    vi.useFakeTimers();
    forgetPathState();
    mocks.pathKinds.mockImplementation(async (paths: string[]) => paths.map((path) => (FILES.has(path) ? "file" : null)));
});

afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
});

async function nextFrame() {
    await act(async () => {
        await vi.advanceTimersByTimeAsync(16);
    });
}

describe("usePathState", () => {
    it("asks about every path on screen in one request and answers each", async () => {
        const a = renderHook(() => usePathState("/repo/a.ts"));
        const gone = renderHook(() => usePathState("/repo/gone.ts"));
        expect(a.result.current).toBeNull();

        await nextFrame();

        expect(mocks.pathKinds).toHaveBeenCalledOnce();
        expect(mocks.pathKinds).toHaveBeenCalledWith(["/repo/a.ts", "/repo/gone.ts"]);
        expect(a.result.current).toBe("file");
        expect(gone.result.current).toBe("missing");
    });

    it("answers from what it already knows without asking again", async () => {
        renderHook(() => usePathState("/repo/a.ts"));
        await nextFrame();

        const again = renderHook(() => usePathState("/repo/a.ts"));

        expect(again.result.current).toBe("file");
        await nextFrame();
        expect(mocks.pathKinds).toHaveBeenCalledOnce();
    });

    it("trusts a missing file only for a few seconds, since the agent may be about to write it", async () => {
        renderHook(() => usePathState("/repo/soon.ts"));
        await nextFrame();
        FILES.add("/repo/soon.ts");
        try {
            expect(renderHook(() => usePathState("/repo/soon.ts")).result.current).toBe("missing");

            await act(async () => {
                await vi.advanceTimersByTimeAsync(5_001);
            });
            const later = renderHook(() => usePathState("/repo/soon.ts"));
            expect(later.result.current).toBeNull();
            await nextFrame();
            expect(later.result.current).toBe("file");
        } finally {
            FILES.delete("/repo/soon.ts");
        }
    });

    it("reads a lookup that failed as a missing file", async () => {
        mocks.pathKinds.mockRejectedValueOnce(new Error("backend gone"));
        const { result } = renderHook(() => usePathState("/repo/a.ts"));

        await nextFrame();

        expect(result.current).toBe("missing");
    });

    it("reads an answer the backend left out as a missing file", async () => {
        mocks.pathKinds.mockResolvedValueOnce([]);
        const { result } = renderHook(() => usePathState("/repo/a.ts"));

        await nextFrame();

        expect(result.current).toBe("missing");
    });

    it("splits more paths than one request takes across several", async () => {
        const paths = Array.from({ length: 300 }, (_, index) => `/repo/many/${index}.ts`);
        const hooks = paths.map((path) => renderHook(() => usePathState(path)));

        await nextFrame();
        await nextFrame();

        expect(mocks.pathKinds.mock.calls.map(([asked]) => asked.length)).toEqual([256, 44]);
        expect(hooks.at(-1)?.result.current).toBe("missing");
    });

    it("drops a path from the request when nothing on screen still wants it", async () => {
        const leaving = renderHook(() => usePathState("/repo/b.ts"));
        renderHook(() => usePathState("/repo/a.ts"));
        leaving.unmount();

        await nextFrame();

        expect(mocks.pathKinds).toHaveBeenCalledWith(["/repo/a.ts"]);
    });

    it("sends no request once every reference has gone", async () => {
        renderHook(() => usePathState("/repo/a.ts")).unmount();

        await nextFrame();

        expect(mocks.pathKinds).not.toHaveBeenCalled();
    });

    it("follows the path it is given and has no answer for no path", async () => {
        const { result, rerender } = renderHook(({ path }: { path: string | null }) => usePathState(path), {
            initialProps: { path: "/repo/a.ts" as string | null },
        });
        await nextFrame();
        expect(result.current).toBe("file");

        rerender({ path: null });
        expect(result.current).toBeNull();
    });

    it("looks again at a path it was told to forget", async () => {
        renderHook(() => usePathState("/repo/a.ts"));
        await nextFrame();
        forgetPathState("/repo/a.ts");

        const again = renderHook(() => usePathState("/repo/a.ts"));
        expect(again.result.current).toBeNull();
        await nextFrame();

        expect(mocks.pathKinds).toHaveBeenCalledTimes(2);
        expect(again.result.current).toBe("file");
    });
});
