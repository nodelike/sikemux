import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { FileTree } from "./FileTree";
import { emit } from "../state/bus";

const { readDirs } = vi.hoisted(() => ({ readDirs: vi.fn() }));
vi.mock("../api/fs", () => ({ fsapi: { readDirs } }));
vi.mock("../state/resources", async (importOriginal) => ({
    ...(await importOriginal<object>()),
    useResourceEnabled: () => ({ data: undefined, status: "ok", refresh: vi.fn() }),
}));
afterEach(cleanup);

const entriesFor = (path: string) =>
    path === "/repo"
        ? ["one", "two"].map((name) => ({ name, path: `/repo/${name}`, is_dir: true }))
        : [{ name: "file.ts", path: `${path}/file.ts`, is_dir: false }];

const requestedPaths = () => readDirs.mock.calls.map((call) => call[0] as string[]);

it("refreshes only affected expanded directories and retains a full-refresh fallback", async () => {
    readDirs.mockImplementation(async (paths: string[]) => paths.map((path) => ({ path, entries: entriesFor(path), error: null })));
    const { findByText } = render(<FileTree cwd="/repo" active activePath={null} onOpenFile={vi.fn()} onKeepFile={vi.fn()} />);
    fireEvent.click(await findByText("one"));
    fireEvent.click(await findByText("two"));
    await waitFor(() => expect(readDirs).toHaveBeenCalledTimes(3));
    readDirs.mockClear();

    act(() => emit({ type: "fs-changed", repo: "/repo", paths: ["one/file.ts"] }));
    await waitFor(() => expect(requestedPaths()).toEqual([["/repo/one"]]));
    readDirs.mockClear();

    act(() => emit({ type: "fs-changed", repo: "/repo", paths: ["one"] }));
    await waitFor(() => expect(requestedPaths()).toEqual([["/repo", "/repo/one"]]));
    readDirs.mockClear();

    act(() => emit({ type: "fs-changed", repo: "/repo" }));
    await waitFor(() => expect(requestedPaths()).toEqual([["/repo", "/repo/one", "/repo/two"]]));
});

it("keeps a directory that failed to read instead of blanking it", async () => {
    readDirs.mockImplementation(async (paths: string[]) => paths.map((path) => ({ path, entries: entriesFor(path), error: null })));
    const { findByText, queryByText } = render(<FileTree cwd="/repo" active activePath={null} onOpenFile={vi.fn()} onKeepFile={vi.fn()} />);
    await findByText("one");

    readDirs.mockImplementation(async (paths: string[]) => paths.map((path) => ({ path, entries: [], error: "gone" })));
    act(() => emit({ type: "fs-changed", repo: "/repo" }));
    await waitFor(() => expect(readDirs).toHaveBeenCalled());

    expect(queryByText("one")).not.toBeNull();
});
