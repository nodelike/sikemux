import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, renderHook, waitFor } from "@testing-library/react";
import { fetchResource, invalidate, peekResource, refetchFailed, resetResourcesForTests, resource, useResource } from "./resources";

function deferred<T>() {
    let resolve!: (value: T) => void;
    const promise = new Promise<T>((res) => {
        resolve = res;
    });
    return { promise, resolve };
}

beforeEach(() => resetResourcesForTests());

describe("resources", () => {
    it("fetches again only what failed and is still on screen", async () => {
        let online = false;
        const failing = vi.fn(async () => {
            if (!online) throw new Error("You're offline");
            return "issues";
        });
        const steady = vi.fn(async () => "fine");
        const shown = resource({ kind: "shown-failure", fetch: failing });
        const hidden = resource({ kind: "hidden-failure", fetch: vi.fn(async () => Promise.reject(new Error("offline"))) });
        const healthy = resource({ kind: "healthy", fetch: steady });

        const failed = renderHook(() => useResource(shown));
        const fine = renderHook(() => useResource(healthy));
        await expect(fetchResource(hidden)).rejects.toThrow();
        await waitFor(() => expect(failed.result.current.status).toBe("error"));
        await waitFor(() => expect(fine.result.current.data).toBe("fine"));
        const hiddenCalls = vi.mocked(hidden.fetch).mock.calls.length;

        online = true;
        act(() => refetchFailed());
        await waitFor(() => expect(failed.result.current.data).toBe("issues"));
        expect(failing).toHaveBeenCalledTimes(2);
        expect(steady).toHaveBeenCalledTimes(1);
        expect(vi.mocked(hidden.fetch).mock.calls.length).toBe(hiddenCalls);
        failed.unmount();
        fine.unmount();
    });

    it("keeps what last loaded, and when, after a refresh fails offline", async () => {
        let online = true;
        const issues = resource({
            kind: "kept-offline",
            fetch: vi.fn(async () => {
                if (!online) throw { category: "offline", message: "Can't reach Jira. Check your connection." };
                return "issues";
            }),
        });
        const shown = renderHook(() => useResource(issues));
        await waitFor(() => expect(shown.result.current.data).toBe("issues"));
        const loadedAt = shown.result.current.loadedAt;
        expect(loadedAt).toBeTypeOf("number");
        expect(shown.result.current.failure).toBeUndefined();

        online = false;
        await act(() => shown.result.current.refresh().catch(() => {}));
        expect(shown.result.current).toMatchObject({
            status: "error",
            data: "issues",
            failure: "offline",
            loadedAt,
            error: "Can't reach Jira. Check your connection.",
        });
        shown.unmount();
    });

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
