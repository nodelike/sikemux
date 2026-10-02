import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
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
    beforeEach(() => {
        vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(800);
        vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(1_000);
    });

    it("renders every changed file in one collapsible stream", () => {
        render(<MergeReview repo="/repo" files={files} onOpenFile={() => {}} onSaved={() => {}} />);

        expect(screen.getByText("3 files")).toBeInTheDocument();
        expect(screen.getByTestId("diff:staged.ts:HEAD::index")).toBeInTheDocument();
        expect(screen.getByTestId("diff:both.ts:HEAD::index")).toBeInTheDocument();
        expect(screen.getByTestId("diff:both.ts::index:working")).toBeInTheDocument();
        expect(screen.getByTestId("diff:working.ts:HEAD:working")).toBeInTheDocument();
        expect(screen.getByText(/^staged$/i)).toBeInTheDocument();
        expect(screen.getByText(/^unstaged$/i)).toBeInTheDocument();
        expect(screen.getAllByLabelText("staged: modified")).toHaveLength(3);
        expect(screen.getAllByLabelText("unstaged: modified")).toHaveLength(3);

        fireEvent.click(screen.getByRole("button", { name: "Collapse both.ts" }));
        expect(screen.getByRole("button", { name: "Expand both.ts" })).toBeInTheDocument();
        expect(screen.queryByTestId("diff:both.ts:HEAD::index")).not.toBeInTheDocument();
        expect(screen.queryByTestId("diff:both.ts::index:working")).not.toBeInTheDocument();
        expect(screen.getByTestId("diff:staged.ts:HEAD::index")).toBeInTheDocument();

        fireEvent.click(screen.getByRole("button", { name: "Collapse all" }));
        expect(screen.getByRole("button", { name: "Collapse all" })).toBeDisabled();
        expect(screen.queryByTestId("diff:staged.ts:HEAD::index")).not.toBeInTheDocument();

        fireEvent.click(screen.getByRole("button", { name: "Expand all" }));
        expect(screen.getByRole("button", { name: "Expand all" })).toBeDisabled();
        expect(screen.getByTestId("diff:working.ts:HEAD:working")).toBeInTheDocument();
    });

    it("keeps file names wired to the editor", () => {
        const onOpenFile = vi.fn();
        render(<MergeReview repo="/repo" files={files} onOpenFile={onOpenFile} onSaved={() => {}} />);

        fireEvent.click(screen.getByRole("button", { name: "staged.ts" }));
        expect(onOpenFile).toHaveBeenCalledWith("/repo/staged.ts");
        expect(screen.getByTestId("diff:staged.ts:HEAD::index")).toBeInTheDocument();
    });

    it("folds a file from anywhere on its header except the name", () => {
        const onOpenFile = vi.fn();
        const { container } = render(<MergeReview repo="/repo" files={files} onOpenFile={onOpenFile} onSaved={() => {}} />);

        fireEvent.click(container.querySelector(".merge-file-header .acc-grow")!);
        expect(screen.queryByTestId("diff:staged.ts:HEAD::index")).not.toBeInTheDocument();
        expect(onOpenFile).not.toHaveBeenCalled();

        fireEvent.click(screen.getByRole("button", { name: "Expand staged.ts" }));
        expect(screen.getByTestId("diff:staged.ts:HEAD::index")).toBeInTheDocument();
    });

    it("only scrolls when the focused file changes, not on every status refresh", () => {
        vi.useFakeTimers();
        const scrollTo = vi.fn();
        const previous = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "scrollTo");
        Object.defineProperty(HTMLElement.prototype, "scrollTo", { value: scrollTo, configurable: true, writable: true });

        const { rerender } = render(<MergeReview repo="/repo" files={files} focusPath="both.ts" onOpenFile={() => {}} onSaved={() => {}} />);
        act(() => void vi.advanceTimersByTime(32));
        const settled = scrollTo.mock.calls.length;
        expect(settled).toBeGreaterThan(0);

        rerender(<MergeReview repo="/repo" files={files.map((f) => ({ ...f }))} focusPath="both.ts" onOpenFile={() => {}} onSaved={() => {}} />);
        act(() => void vi.advanceTimersByTime(32));
        expect(scrollTo.mock.calls.length).toBe(settled);

        rerender(<MergeReview repo="/repo" files={files} focusPath="working.ts" onOpenFile={() => {}} onSaved={() => {}} />);
        act(() => void vi.advanceTimersByTime(32));
        expect(scrollTo.mock.calls.length).toBeGreaterThan(settled);

        if (previous) Object.defineProperty(HTMLElement.prototype, "scrollTo", previous);
        else Reflect.deleteProperty(HTMLElement.prototype, "scrollTo");
        vi.useRealTimers();
    });

    it("bounds mounted files and diffs in a 1,000-file review", () => {
        const manyFiles = Array.from<unknown, GitFile>({ length: 1_000 }, (_, index) => ({
            path: `src/file-${index}.ts`,
            index: " ",
            worktree: "M",
        }));

        const { container } = render(<MergeReview repo="/repo" files={manyFiles} onOpenFile={() => {}} onSaved={() => {}} />);

        expect(screen.getByText("1000 files")).toBeInTheDocument();
        expect(container.querySelector(".merge-review-virtual")).toBeInTheDocument();
        expect(container.querySelectorAll(".merge-review-item").length).toBeLessThan(12);
        expect(container.querySelectorAll('[data-testid^="diff:"]').length).toBeLessThan(12);
        expect(screen.getByRole("button", { name: "src/file-0.ts" })).toBeInTheDocument();
        expect(screen.queryByRole("button", { name: "src/file-999.ts" })).not.toBeInTheDocument();

        fireEvent.click(screen.getByRole("button", { name: "Collapse all" }));
        expect(screen.getByRole("button", { name: "Collapse all" })).toBeDisabled();
        expect(container.querySelectorAll('[data-testid^="diff:"]')).toHaveLength(0);
        expect(container.querySelectorAll(".merge-review-item").length).toBeLessThan(40);
    });

    it("contains loading diffs within their slots while measuring their full growing height", () => {
        const observers = new Map<Element, ResizeObserverCallback>();
        vi.stubGlobal(
            "ResizeObserver",
            class {
                constructor(private callback: ResizeObserverCallback) {}
                observe(element: Element) {
                    observers.set(element, this.callback);
                }
                unobserve(element: Element) {
                    observers.delete(element);
                }
                disconnect() {}
            },
        );
        vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockImplementation(function (this: HTMLElement) {
            return this.classList.contains("merge-review-list") ? 800 : 250;
        });
        vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(1_000);
        const manyFiles = Array.from<unknown, GitFile>({ length: 30 }, (_, index) => ({
            path: `file-${index}.ts`,
            index: " ",
            worktree: "M",
        }));
        const { container } = render(<MergeReview repo="/repo" files={manyFiles} onOpenFile={() => {}} onSaved={() => {}} />);
        const slot = container.querySelector<HTMLElement>(".merge-review-virtual-item")!;
        const content = slot.firstElementChild as HTMLElement;

        expect(slot).toHaveStyle({ height: "250px", overflow: "clip" });
        expect(content.style.height).toBe("");
        expect(observers.has(content)).toBe(true);
        expect(observers.has(slot)).toBe(false);

        act(() => {
            observers.get(content)!(
                [{ target: content, borderBoxSize: [{ blockSize: 1_200, inlineSize: 1_000 }] } as unknown as ResizeObserverEntry],
                {} as ResizeObserver,
            );
        });

        expect(slot).toHaveStyle({ height: "1200px", overflow: "clip" });
        expect(slot.nextElementSibling).toHaveStyle({ top: "1200px" });
        expect(screen.getByTestId("diff:file-0.ts:HEAD:working")).toBeInTheDocument();
    });

    it("shows each file's own actions and follows a file whose status changed", () => {
        const actions = (file: GitFile) => <button type="button">{`Act on ${file.path}`}</button>;
        const { rerender } = render(
            <MergeReview repo="/repo" files={files} focusPath="working.ts" onOpenFile={() => {}} onSaved={() => {}} fileActions={actions} />,
        );
        expect(screen.getByRole("button", { name: "Act on both.ts" })).toBeInTheDocument();
        expect(screen.getByTestId("diff:working.ts:HEAD:working")).toHaveAttribute("data-editable", "true");
        expect(screen.getByTestId("diff:staged.ts:HEAD::index")).toHaveAttribute("data-editable", "false");

        const staged = files.map((f) => (f.path === "working.ts" ? { ...f, index: "M", worktree: " " } : f));
        rerender(<MergeReview repo="/repo" files={staged} focusPath="working.ts" onOpenFile={() => {}} onSaved={() => {}} fileActions={actions} />);
        expect(screen.getByTestId("diff:working.ts:HEAD::index")).toBeInTheDocument();

        rerender(<MergeReview repo="/repo" files={staged.slice(0, 1)} onOpenFile={() => {}} onSaved={() => {}} fileActions={actions} />);
        expect(screen.getByText("1 file")).toBeInTheDocument();
        expect(screen.queryByRole("button", { name: "Act on both.ts" })).toBeNull();
    });
});
