import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DiffRow } from "../api/git";
import { themeById } from "../themes";

const mocks = vi.hoisted(() => ({
    fileAt: vi.fn(),
    fileDiff: vi.fn(),
    readTextFileLimited: vi.fn(),
    writeFile: vi.fn(),
    currentTheme: vi.fn(),
    themeListeners: new Set<(theme: ReturnType<typeof themeById>) => void>(),
    mergeProps: null as Record<string, any> | null,
}));

vi.mock("../api/git", async (importOriginal) => ({
    ...(await importOriginal<typeof import("../api/git")>()),
    git: { fileAt: mocks.fileAt, fileDiff: mocks.fileDiff },
}));
vi.mock("../api/fs", () => ({
    fsapi: {
        readTextFileLimited: mocks.readTextFileLimited,
        writeFile: mocks.writeFile,
    },
}));
vi.mock("../themes/bus", () => ({
    currentTheme: mocks.currentTheme,
    subscribeTheme: (listener: (theme: ReturnType<typeof themeById>) => void) => {
        mocks.themeListeners.add(listener);
        return () => mocks.themeListeners.delete(listener);
    },
}));
vi.mock("./DiffMergeEditor", () => ({
    default: (props: Record<string, any>) => {
        mocks.mergeProps = props;
        return <div data-testid="merge-editor" />;
    },
}));

import { DiffEditor, invalidateDiffContentCache } from "./DiffEditor";
import { emit } from "../state/bus";

const ROWS: DiffRow[] = [
    [0, 1, "import { a } from './a';"],
    [2, 2, "const value = 1;"],
    [1, 2, "const value = 2;"],
    [3, 12, ""],
    [0, 20, "export default value;"],
];

beforeEach(() => {
    invalidateDiffContentCache();
    mocks.fileAt.mockReset().mockResolvedValue("const value = 1;\n");
    mocks.fileDiff.mockReset().mockResolvedValue(ROWS);
    mocks.readTextFileLimited.mockReset().mockResolvedValue("const value = 2;\n");
    mocks.writeFile.mockReset().mockResolvedValue(undefined);
    mocks.currentTheme.mockReset().mockReturnValue(themeById("aura"));
    mocks.themeListeners.clear();
    mocks.mergeProps = null;
});

afterEach(cleanup);

describe("DiffEditor", () => {
    it("draws the rows the native side worked out, numbered and marked by kind", async () => {
        const { container } = render(<DiffEditor repo="/repo" path="src/app.ts" baseRev="HEAD" headRev=":index" editable={false} />);

        await waitFor(() => expect(container.querySelectorAll(".diff-row")).toHaveLength(5));
        expect(mocks.fileDiff).toHaveBeenCalledWith("/repo", "src/app.ts", "HEAD", ":index", false);
        const rows = [...container.querySelectorAll(".diff-row")];
        expect(rows.map((row) => row.getAttribute("data-kind"))).toEqual(["context", "deleted", "added", "hidden", "context"]);
        expect(rows[1].querySelector(".diff-num")?.textContent).toBe("2");
        expect(rows[2].querySelector(".diff-code")?.textContent).toBe("const value = 2;");
        expect(container.querySelector(".diff-view")?.classList.contains("tinted")).toBe(true);
    });

    it("runs to the working tree when there is no head revision", async () => {
        const { container } = render(<DiffEditor repo="/repo" path="src/app.ts" baseRev="HEAD" editable={false} />);
        await waitFor(() => expect(container.querySelector(".diff-view")).toBeInTheDocument());
        expect(mocks.fileDiff).toHaveBeenCalledWith("/repo", "src/app.ts", "HEAD", null, false);
    });

    it("shows the lines a hidden row stands for when it is clicked", async () => {
        const { container, getByRole } = render(<DiffEditor repo="/repo" path="src/app.ts" baseRev="HEAD" headRev=":index" editable={false} />);
        await waitFor(() => expect(getByRole("button", { name: "Show 12 unchanged lines" })).toBeInTheDocument());

        mocks.fileDiff.mockResolvedValue(ROWS.filter((row) => row[0] !== 3));
        fireEvent.click(getByRole("button", { name: "Show 12 unchanged lines" }));

        await waitFor(() => expect(mocks.fileDiff).toHaveBeenLastCalledWith("/repo", "src/app.ts", "HEAD", ":index", true));
        await waitFor(() => expect(container.querySelectorAll(".diff-row")).toHaveLength(4));
    });

    it("tints changed rows only on a dark theme", async () => {
        const { container } = render(<DiffEditor repo="/repo" path="src/app.ts" baseRev="HEAD" headRev="main" editable={false} />);
        await waitFor(() => expect(container.querySelector(".diff-view.tinted")).toBeInTheDocument());

        const day = themeById("aura-day");
        mocks.currentTheme.mockReturnValue(day);
        act(() => mocks.themeListeners.forEach((listener) => listener(day)));

        expect(container.querySelector(".diff-view")?.classList.contains("tinted")).toBe(false);
    });

    it("draws nothing for a file that did not change", async () => {
        mocks.fileDiff.mockResolvedValue([]);
        const { container } = render(<DiffEditor repo="/repo" path="src/same.ts" baseRev="HEAD" headRev=":index" editable={false} />);
        await waitFor(() => expect(container.querySelector(".diff-editor-loading")).not.toBeInTheDocument());
        expect(container.querySelector(".diff-view")).not.toBeInTheDocument();
    });

    it("says why a diff cannot be shown", async () => {
        mocks.fileDiff.mockRejectedValue(new Error("src/logo.png is binary; inline diff is disabled."));
        const { findByText } = render(<DiffEditor repo="/repo" path="src/logo.png" baseRev="HEAD" headRev=":index" editable={false} />);
        expect(await findByText("x src/logo.png is binary; inline diff is disabled.")).toBeInTheDocument();
    });

    it("edits the working file against its base and saves it with Cmd+S", async () => {
        const onSaved = vi.fn();
        const { container, findByTestId } = render(<DiffEditor repo="/repo" path="src/app.ts" baseRev="HEAD" editable onSaved={onSaved} />);

        await findByTestId("merge-editor");
        expect(mocks.fileDiff).not.toHaveBeenCalled();
        expect(mocks.mergeProps).toMatchObject({ base: "const value = 1;\n", head: "const value = 2;\n", path: "src/app.ts", tinted: true });

        act(() => mocks.mergeProps?.onChange("const value = 3;\n"));
        fireEvent.keyDown(container.querySelector(".diff-editor")!, { key: "s", metaKey: true });

        await waitFor(() => expect(mocks.writeFile).toHaveBeenCalledWith("/repo/src/app.ts", "const value = 3;\n"));
        expect(onSaved).toHaveBeenCalledOnce();
    });

    it("refuses to edit a file that looks binary", async () => {
        mocks.readTextFileLimited.mockResolvedValue("PNG\0\0\0");
        const { findByText } = render(<DiffEditor repo="/repo" path="logo.png" baseRev="HEAD" editable />);
        expect(await findByText("x logo.png looks binary; inline diff is disabled.")).toBeInTheDocument();
    });

    it("deduplicates simultaneous reads", async () => {
        render(
            <>
                <DiffEditor repo="/repo" path="src/shared.ts" baseRev="HEAD" headRev=":index" editable={false} />
                <DiffEditor repo="/repo" path="src/shared.ts" baseRev="HEAD" headRev=":index" editable={false} />
            </>,
        );

        await waitFor(() => expect(document.querySelectorAll(".diff-view")).toHaveLength(2));
        expect(mocks.fileDiff).toHaveBeenCalledTimes(1);
    });

    it("reuses completed reads across virtualized remounts and invalidates them by repository", async () => {
        const first = render(<DiffEditor repo="/repo" path="src/revisit.ts" baseRev="HEAD" headRev=":index" editable={false} />);
        await waitFor(() => expect(first.container.querySelector(".diff-view")).toBeInTheDocument());
        first.unmount();

        const second = render(<DiffEditor repo="/repo" path="src/revisit.ts" baseRev="HEAD" headRev=":index" editable={false} />);
        await waitFor(() => expect(second.container.querySelector(".diff-view")).toBeInTheDocument());
        expect(mocks.fileDiff).toHaveBeenCalledTimes(1);
        second.unmount();

        invalidateDiffContentCache("/repo");
        const third = render(<DiffEditor repo="/repo" path="src/revisit.ts" baseRev="HEAD" headRev=":index" editable={false} />);
        await waitFor(() => expect(third.container.querySelector(".diff-view")).toBeInTheDocument());
        expect(mocks.fileDiff).toHaveBeenCalledTimes(2);
    });

    it("reads the diff again when its file changes on disk, and not for other files", async () => {
        const { container } = render(<DiffEditor repo="/repo" path="src/app.ts" baseRev="HEAD" editable={false} />);
        await waitFor(() => expect(container.querySelector(".diff-view")).toBeInTheDocument());
        expect(mocks.fileDiff).toHaveBeenCalledTimes(1);

        act(() => emit({ type: "fs-changed", repo: "/repo", paths: ["src/other.ts"] }));
        act(() => emit({ type: "fs-changed", repo: "/elsewhere", paths: ["src/app.ts"] }));
        expect(mocks.fileDiff).toHaveBeenCalledTimes(1);

        act(() => emit({ type: "fs-changed", repo: "/repo", paths: ["src/app.ts"] }));
        await waitFor(() => expect(mocks.fileDiff).toHaveBeenCalledTimes(2));

        act(() => emit({ type: "git-refresh", repo: "/repo" }));
        await waitFor(() => expect(mocks.fileDiff).toHaveBeenCalledTimes(3));
    });

    it("keeps a committed diff as it is when the working tree changes", async () => {
        const { container } = render(<DiffEditor repo="/repo" path="src/app.ts" baseRev="abc~1" headRev="abc" editable={false} />);
        await waitFor(() => expect(container.querySelector(".diff-view")).toBeInTheDocument());
        act(() => emit({ type: "fs-changed", repo: "/repo", paths: ["src/app.ts"] }));
        act(() => emit({ type: "git-refresh", repo: "/repo" }));
        expect(mocks.fileDiff).toHaveBeenCalledTimes(1);
    });

    it("picks up an outside edit in the editable diff unless there is unsaved typing", async () => {
        const { findByTestId } = render(<DiffEditor repo="/repo" path="src/app.ts" baseRev="HEAD" editable />);
        await findByTestId("merge-editor");

        mocks.readTextFileLimited.mockResolvedValue("const value = 4;\n");
        act(() => emit({ type: "fs-changed", repo: "/repo", paths: ["src/app.ts"] }));
        await waitFor(() => expect(mocks.mergeProps?.head).toBe("const value = 4;\n"));

        act(() => mocks.mergeProps?.onChange("const value = 5;\n"));
        mocks.readTextFileLimited.mockResolvedValue("const value = 6;\n");
        act(() => emit({ type: "fs-changed", repo: "/repo", paths: ["src/app.ts"] }));
        await new Promise((resolve) => setTimeout(resolve, 20));
        expect(mocks.mergeProps?.head).toBe("const value = 4;\n");
    });

    it("edits a file deleted from the working tree as empty, but reports any other read failure", async () => {
        mocks.readTextFileLimited.mockRejectedValue(new Error("No such file or directory (os error 2)"));
        const first = render(<DiffEditor repo="/repo" path="gone.ts" baseRev="HEAD" editable />);
        await first.findByTestId("merge-editor");
        expect(mocks.mergeProps?.head).toBe("");
        first.unmount();

        mocks.readTextFileLimited.mockRejectedValue(new Error("permission denied"));
        const second = render(<DiffEditor repo="/repo" path="locked.ts" baseRev="HEAD" editable />);
        expect(await second.findByText("x permission denied")).toBeInTheDocument();
    });

    it("edits against a head revision instead of the working file", async () => {
        mocks.fileAt.mockImplementation(async (_repo: string, rev: string) => `at ${rev}\n`);
        const { findByTestId, container } = render(<DiffEditor repo="/repo" path="src/app.ts" baseRev="HEAD" headRev="abc" editable autoHeight />);
        await findByTestId("merge-editor");
        expect(mocks.readTextFileLimited).not.toHaveBeenCalled();
        expect(mocks.mergeProps).toMatchObject({ base: "at HEAD\n", head: "at abc\n" });
        expect(container.querySelector(".diff-editor")).toHaveClass("auto");
    });

    it("refuses to edit a file too large to diff inline, and does not keep it cached", async () => {
        mocks.fileAt.mockResolvedValue("x".repeat(33 * 1024 * 1024));
        const first = render(<DiffEditor repo="/repo" path="huge.log" baseRev="HEAD" headRev="abc" editable />);
        expect(await first.findByText("x huge.log is too large for inline diff (33.0 MB).")).toBeInTheDocument();
        first.unmount();

        const reads = mocks.fileAt.mock.calls.length;
        const second = render(<DiffEditor repo="/repo" path="huge.log" baseRev="HEAD" headRev="abc" editable />);
        await second.findByText(/too large/);
        expect(mocks.fileAt.mock.calls.length).toBeGreaterThan(reads);
    });

    it("treats text full of replacement characters as binary", async () => {
        mocks.readTextFileLimited.mockResolvedValue("\ufffd".repeat(9));
        const { findByText } = render(<DiffEditor repo="/repo" path="data.bin" baseRev="HEAD" editable />);
        expect(await findByText("x data.bin looks binary; inline diff is disabled.")).toBeInTheDocument();
    });

    it("reads a failed diff again rather than remembering the failure", async () => {
        mocks.fileDiff.mockRejectedValueOnce(new Error("index.lock exists"));
        const first = render(<DiffEditor repo="/repo" path="src/app.ts" baseRev="HEAD" headRev=":index" editable={false} />);
        expect(await first.findByText("x index.lock exists")).toBeInTheDocument();
        first.unmount();

        const second = render(<DiffEditor repo="/repo" path="src/app.ts" baseRev="HEAD" headRev=":index" editable={false} />);
        await waitFor(() => expect(second.container.querySelector(".diff-view")).toBeInTheDocument());
        expect(mocks.fileDiff).toHaveBeenCalledTimes(2);
    });

    it("saves only on the save shortcut, and never from a read-only diff", async () => {
        const readOnly = render(<DiffEditor repo="/repo" path="src/app.ts" baseRev="HEAD" headRev=":index" editable={false} />);
        await waitFor(() => expect(readOnly.container.querySelector(".diff-view")).toBeInTheDocument());
        fireEvent.keyDown(readOnly.container.querySelector(".diff-editor")!, { key: "s", metaKey: true });
        readOnly.unmount();

        const editable = render(<DiffEditor repo="/repo" path="src/app.ts" baseRev="HEAD" editable />);
        await editable.findByTestId("merge-editor");
        const root = editable.container.querySelector(".diff-editor")!;
        fireEvent.keyDown(root, { key: "s" });
        fireEvent.keyDown(root, { key: "a", ctrlKey: true });
        await new Promise((resolve) => setTimeout(resolve, 0));
        expect(mocks.writeFile).not.toHaveBeenCalled();

        fireEvent.keyDown(root, { key: "S", ctrlKey: true });
        await waitFor(() => expect(mocks.writeFile).toHaveBeenCalledOnce());
    });

    it("reads again after a change anywhere when the event names no repository or files", async () => {
        const { container } = render(<DiffEditor repo="/repo" path="src/app.ts" baseRev="HEAD" editable={false} />);
        await waitFor(() => expect(container.querySelector(".diff-view")).toBeInTheDocument());

        act(() => emit({ type: "fs-changed", repo: "" }));
        await waitFor(() => expect(mocks.fileDiff).toHaveBeenCalledTimes(2));
        act(() => emit({ type: "git-refresh", repo: "" }));
        await waitFor(() => expect(mocks.fileDiff).toHaveBeenCalledTimes(3));
        act(() => emit({ type: "git-refresh", repo: "/elsewhere" }));
        await new Promise((resolve) => setTimeout(resolve, 0));
        expect(mocks.fileDiff).toHaveBeenCalledTimes(3);
    });

    it("drops a read that finishes after it was invalidated", async () => {
        let finish!: (rows: DiffRow[]) => void;
        mocks.fileDiff.mockImplementationOnce(() => new Promise<DiffRow[]>((resolve) => (finish = resolve)));
        const first = render(<DiffEditor repo="/repo" path="src/slow.ts" baseRev="HEAD" headRev="abc" editable={false} />);
        invalidateDiffContentCache("/repo");
        await act(async () => finish(ROWS));
        await waitFor(() => expect(first.container.querySelector(".diff-view")).toBeInTheDocument());
        first.unmount();

        const second = render(<DiffEditor repo="/repo" path="src/slow.ts" baseRev="HEAD" headRev="abc" editable={false} />);
        await waitFor(() => expect(second.container.querySelector(".diff-view")).toBeInTheDocument());
        expect(mocks.fileDiff).toHaveBeenCalledTimes(2);
    });
});
