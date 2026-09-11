import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { invalidate, resource, useCachedResourceEnabled } from "./resources";

afterEach(cleanup);
it("retains cached data while paused without fetching until reactivated", async () => {
    const first = { files: ["a.ts"] };
    const second = { files: ["b.ts"] };
    const fetch = vi.fn().mockResolvedValueOnce(first).mockResolvedValueOnce(second);
    const definition = resource({ kind: "test.retained.git", fetch, staleAfterMs: 0 });
    const { result, rerender } = renderHook(({ active }) => useCachedResourceEnabled(active, definition), { initialProps: { active: true } });
    await waitFor(() => expect(result.current.data).toBe(first));
    rerender({ active: false });
    expect(result.current.data).toBe(first);
    act(() => invalidate((kind) => kind === definition.kind));
    expect(fetch).toHaveBeenCalledTimes(1);
    rerender({ active: true });
    expect(result.current.data).toBe(first);
    await waitFor(() => expect(result.current.data).toBe(second));
    expect(fetch).toHaveBeenCalledTimes(2);
});
it("never returns another repository's cached data", async () => {
    const fetch = vi.fn(async (repo: string) => ({ repo }));
    const definition = resource({ kind: "test.retained.repository", fetch });
    const { result, rerender } = renderHook(({ repo, active }) => useCachedResourceEnabled(active, definition, repo), {
        initialProps: { repo: "one", active: true },
    });
    await waitFor(() => expect(result.current.data).toEqual({ repo: "one" }));
    rerender({ repo: "two", active: false });
    expect(result.current.data).toBeUndefined();
    expect(fetch).toHaveBeenCalledTimes(1);
});
