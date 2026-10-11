import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { FileTree } from "./FileTree";
import { emit } from "../state/bus";

const { readDirs, overview } = vi.hoisted(() => ({ readDirs: vi.fn(), overview: { data: undefined as unknown } }));
vi.mock("../api/fs", () => ({ fsapi: { readDirs } }));
vi.mock("../state/resources", async (importOriginal) => ({
    ...(await importOriginal<object>()),
    useResourceEnabled: () => ({ data: overview.data, status: "ok", refresh: vi.fn() }),
}));
afterEach(() => {
    cleanup();
    overview.data = undefined;
});

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

it("dims what git ignores and rereads every folder under a .gitignore that changed", async () => {
    readDirs.mockImplementation(async (paths: string[]) =>
        paths.map((path) => ({
            path,
            entries:
                path === "/repo"
                    ? [
                          { name: "dist", path: "/repo/dist", is_dir: true, ignored: true },
                          { name: "src", path: "/repo/src", is_dir: true },
                      ]
                    : [{ name: `${path.slice(6)}.ts`, path: `${path}/${path.slice(6)}.ts`, is_dir: false }],
            error: null,
        })),
    );
    const { findByText } = render(<FileTree cwd="/repo" active activePath={null} onOpenFile={vi.fn()} onKeepFile={vi.fn()} />);
    expect((await findByText("dist")).closest(".tree-row")?.classList.contains("ignored")).toBe(true);
    fireEvent.click(await findByText("src"));
    expect((await findByText("src.ts")).closest(".tree-row")?.classList.contains("ignored")).toBe(false);
    expect((await findByText("src")).closest(".tree-row")?.classList.contains("ignored")).toBe(false);
    readDirs.mockClear();

    act(() => emit({ type: "fs-changed", repo: "/repo", paths: [".gitignore"] }));
    await waitFor(() => expect(requestedPaths()).toEqual([["/repo", "/repo/src"]]));
});

it("colours every folder above a changed file, with a dot, and leaves untouched ones plain", async () => {
    overview.data = { status: { files: [{ path: "src/chat/longText.ts", index: " ", worktree: "M" }] } };
    readDirs.mockImplementation(async (paths: string[]) =>
        paths.map((path) => ({
            path,
            entries:
                path === "/repo"
                    ? [
                          { name: "docs", path: "/repo/docs", is_dir: true },
                          { name: "src", path: "/repo/src", is_dir: true },
                      ]
                    : path === "/repo/src"
                      ? [{ name: "chat", path: "/repo/src/chat", is_dir: true }]
                      : [{ name: "longText.ts", path: "/repo/src/chat/longText.ts", is_dir: false }],
            error: null,
        })),
    );
    const { findByText } = render(<FileTree cwd="/repo" active activePath={null} onOpenFile={vi.fn()} onKeepFile={vi.fn()} />);
    const src = (await findByText("src")).closest(".tree-row")!;
    expect(src.classList.contains("git-m")).toBe(true);
    expect(src.querySelector(".tree-git-dot")).toHaveAttribute("title", "Holds modified files");
    expect((await findByText("docs")).closest(".tree-row")!.className).not.toContain("git-");

    fireEvent.click(src);
    expect((await findByText("chat")).closest(".tree-row")!.classList.contains("git-m")).toBe(true);
});
