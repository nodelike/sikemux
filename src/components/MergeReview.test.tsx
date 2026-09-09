import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { GitFile } from "../api/git";

vi.mock("./DiffEditor", () => ({
    DiffEditor: ({ path, baseRev, headRev, editable }: { path: string; baseRev: string; headRev?: string; editable: boolean }) => (
        <div data-testid={`diff:${path}:${baseRev}:${headRev ?? "working"}`} data-editable={editable} />
    ),
}));

import { MergeReview } from "./MergeReview";

afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
});

const files: GitFile[] = [
    { path: "staged.ts", index: "M", worktree: " " },
    { path: "both.ts", index: "M", worktree: "M" },
    { path: "working.ts", index: " ", worktree: "M" },
];

describe("MergeReview", () => {
    it("renders every changed file in one collapsible stream", () => {
        render(<MergeReview repo="/repo" files={files} onOpenFile={() => {}} onSaved={() => {}} />);

        expect(screen.getByText("3 files · 3 expanded")).toBeInTheDocument();
        expect(screen.getByTestId("diff:staged.ts:HEAD::index")).toBeInTheDocument();
        expect(screen.getByTestId("diff:both.ts:HEAD::index")).toBeInTheDocument();
        expect(screen.getByTestId("diff:both.ts::index:working")).toBeInTheDocument();
        expect(screen.getByTestId("diff:working.ts:HEAD:working")).toBeInTheDocument();
        expect(screen.queryByText(/^staged$/i)).not.toBeInTheDocument();
        expect(screen.queryByText(/^unstaged$/i)).not.toBeInTheDocument();
        expect(screen.getAllByLabelText("Index status: M")).toHaveLength(3);
        expect(screen.getAllByLabelText("Working tree status: M")).toHaveLength(3);

        fireEvent.click(screen.getByRole("button", { name: "Collapse both.ts" }));
        expect(screen.getByText("3 files · 2 expanded")).toBeInTheDocument();
        expect(screen.queryByTestId("diff:both.ts:HEAD::index")).not.toBeInTheDocument();
        expect(screen.queryByTestId("diff:both.ts::index:working")).not.toBeInTheDocument();
        expect(screen.getByTestId("diff:staged.ts:HEAD::index")).toBeInTheDocument();

        fireEvent.click(screen.getByRole("button", { name: "collapse all" }));
        expect(screen.getByText("3 files · 0 expanded")).toBeInTheDocument();
        expect(screen.queryByTestId("diff:staged.ts:HEAD::index")).not.toBeInTheDocument();

        fireEvent.click(screen.getByRole("button", { name: "expand all" }));
        expect(screen.getByText("3 files · 3 expanded")).toBeInTheDocument();
        expect(screen.getByTestId("diff:working.ts:HEAD:working")).toBeInTheDocument();
    });

    it("keeps file names wired to the editor", () => {
        const onOpenFile = vi.fn();
        render(<MergeReview repo="/repo" files={files} onOpenFile={onOpenFile} onSaved={() => {}} />);

        fireEvent.click(screen.getByRole("button", { name: "staged.ts" }));
        expect(onOpenFile).toHaveBeenCalledWith("/repo/staged.ts");
    });

    it("bounds mounted files and diffs in a 1,000-file review", () => {
        vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(800);
        vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(1_000);
        const manyFiles = Array.from<unknown, GitFile>({ length: 1_000 }, (_, index) => ({
            path: `src/file-${index}.ts`,
            index: " ",
            worktree: "M",
        }));

        const { container } = render(<MergeReview repo="/repo" files={manyFiles} onOpenFile={() => {}} onSaved={() => {}} />);

        expect(screen.getByText("1000 files · 1000 expanded")).toBeInTheDocument();
        expect(container.querySelector(".merge-review-virtual")).toBeInTheDocument();
        expect(container.querySelectorAll(".merge-review-item").length).toBeLessThan(12);
        expect(container.querySelectorAll('[data-testid^="diff:"]').length).toBeLessThan(12);
        expect(screen.getByRole("button", { name: "src/file-0.ts" })).toBeInTheDocument();
        expect(screen.queryByRole("button", { name: "src/file-999.ts" })).not.toBeInTheDocument();

        fireEvent.click(screen.getByRole("button", { name: "collapse all" }));
        expect(screen.getByText("1000 files · 0 expanded")).toBeInTheDocument();
        expect(container.querySelectorAll('[data-testid^="diff:"]')).toHaveLength(0);
        expect(container.querySelectorAll(".merge-review-item").length).toBeLessThan(40);
    });
});
