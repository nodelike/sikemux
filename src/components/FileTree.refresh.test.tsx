import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { FileTree } from "./FileTree";
import { emit } from "../state/bus";

const { readDir } = vi.hoisted(() => ({ readDir: vi.fn() }));
vi.mock("../api/fs", () => ({ fsapi: { readDir } }));
vi.mock("../state/resources", async (importOriginal) => ({
    ...(await importOriginal<object>()),
    useResourceEnabled: () => ({ data: undefined, status: "ok", refresh: vi.fn() }),
}));
afterEach(cleanup);

it("refreshes only affected expanded directories and retains a full-refresh fallback", async () => {
    readDir.mockImplementation(async (path: string) =>
        path === "/repo"
            ? ["one", "two"].map((name) => ({ name, path: `/repo/${name}`, is_dir: true }))
            : [{ name: "file.ts", path: `${path}/file.ts`, is_dir: false }],
    );
    const { findByText } = render(<FileTree cwd="/repo" active activePath={null} onOpenFile={vi.fn()} />);
    fireEvent.click(await findByText("one"));
    fireEvent.click(await findByText("two"));
    await waitFor(() => expect(readDir).toHaveBeenCalledTimes(3));
    readDir.mockClear();

    act(() => emit({ type: "fs-changed", repo: "/repo", paths: ["one/file.ts"] }));
    await waitFor(() => expect(readDir.mock.calls).toEqual([["/repo/one"]]));
    readDir.mockClear();

    act(() => emit({ type: "fs-changed", repo: "/repo", paths: ["one"] }));
    await waitFor(() => expect(readDir.mock.calls).toEqual([["/repo"], ["/repo/one"]]));
    readDir.mockClear();

    act(() => emit({ type: "fs-changed", repo: "/repo" }));
    await waitFor(() => expect(readDir.mock.calls).toEqual([["/repo"], ["/repo/one"], ["/repo/two"]]));
});
