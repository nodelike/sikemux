import { cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { FileTree } from "./FileTree";

const { readDir } = vi.hoisted(() => ({ readDir: vi.fn() }));
vi.mock("../api/fs", () => ({ fsapi: { readDir } }));
vi.mock("../state/resources", async (importOriginal) => ({
    ...(await importOriginal<object>()),
    useResourceEnabled: () => ({ data: undefined, status: "ok", refresh: vi.fn() }),
}));
vi.mock("@tanstack/react-virtual", () => ({
    useVirtualizer: ({ count }: { count: number }) => ({
        getVirtualItems: () =>
            Array.from({ length: Math.min(count, 30) }, (_, index) => ({ index, key: index, start: index * 23, size: 23, end: (index + 1) * 23 })),
        getTotalSize: () => count * 23,
        scrollToIndex: vi.fn(),
    }),
}));

afterEach(cleanup);

it("keeps a large expanded directory to a small mounted row window", async () => {
    readDir.mockResolvedValue(
        Array.from({ length: 1_000 }, (_, index) => ({ name: `file-${index}.ts`, path: `/repo/file-${index}.ts`, is_dir: false })),
    );

    const { container } = render(<FileTree cwd="/repo" active activePath={null} onOpenFile={vi.fn()} />);

    await waitFor(() => expect(container.querySelectorAll('[role="treeitem"]')).toHaveLength(30));
    expect(container.textContent).toContain("file-0.ts");
    expect(container.textContent).not.toContain("file-999.ts");
});
