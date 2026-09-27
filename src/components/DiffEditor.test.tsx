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
});
