import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, renderHook, waitFor } from "@testing-library/react";
import { fetchResource, invalidate, peekResource, resetResourcesForTests, resource, useResource } from "./resources";

function deferred<T>() {
    let resolve!: (value: T) => void;
    const promise = new Promise<T>((res) => {
        resolve = res;
    });
    return { promise, resolve };
}

beforeEach(() => resetResourcesForTests());

describe("resources", () => {
    it("coalesces a subscribed invalidation storm into one follow-up fetch", async () => {
        const stale = deferred<string>();
        const fetch = vi
            .fn()
            .mockImplementationOnce(() => stale.promise)
            .mockResolvedValue("fresh");
        const def = resource({ kind: "storm", fetch });
        const { result, unmount } = renderHook(() => useResource(def, "/repo"));
        act(() => {
            for (let index = 0; index < 100; index++) invalidate((kind) => kind === def.kind);
        });
        expect(fetch).toHaveBeenCalledTimes(1);
        await act(async () => stale.resolve("stale"));
        await waitFor(() => expect(result.current.data).toBe("fresh"));
        expect(fetch).toHaveBeenCalledTimes(2);
        unmount();
    });

    it("uses collision-free keys for typed and delimited arguments", async () => {
        const fetch = vi.fn(async (...args: unknown[]) => args.join(","));
        const def = resource({ kind: "key-test", fetch });

        await Promise.all([
            fetchResource(def, "a|b", "c"),
            fetchResource(def, "a", "b|c"),
            fetchResource(def, 1),
            fetchResource(def, "1"),
            fetchResource(def, null),
            fetchResource(def, undefined),
        ]);
        expect(fetch).toHaveBeenCalledTimes(6);
    });

    it("distinguishes sparse arrays while canonicalizing plain object key order", async () => {
        const fetch = vi.fn(async (arg: unknown) => arg);
        const def = resource({ kind: "structured-key-test", fetch });
        const sparse = new Array(1);

        await fetchResource(def, sparse);
        await fetchResource(def, [undefined]);
        await Promise.all([fetchResource(def, { a: 1, b: 2 }), fetchResource(def, { b: 2, a: 1 })]);

        expect(fetch).toHaveBeenCalledTimes(3);
    });

    it("does not let an invalidated in-flight generation commit stale data", async () => {
        const stale = deferred<string>();
        const fresh = deferred<string>();
        const fetch = vi
            .fn()
            .mockImplementationOnce(() => stale.promise)
            .mockImplementationOnce(() => fresh.promise);
        const def = resource({ kind: "generation-test", fetch });

        const first = fetchResource(def, "x");
        invalidate((kind) => kind === def.kind);
        const second = fetchResource(def, "x");
        for (let index = 0; index < 100; index++) invalidate((kind) => kind === def.kind);
        expect(fetch).toHaveBeenCalledTimes(1);

        stale.resolve("stale");
        await expect(first).resolves.toBe("stale");
        expect(peekResource(def, "x")).toBeUndefined();
        fresh.resolve("fresh");
        await expect(second).resolves.toBe("fresh");
        expect(peekResource(def, "x")).toBe("fresh");
        expect(fetch).toHaveBeenCalledTimes(2);
    });
});
