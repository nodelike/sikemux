import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { commitFiles, commitLines } = vi.hoisted(() => ({
    commitFiles: vi.fn(),
    commitLines: vi.fn(() => Promise.resolve({ authors: [], files: [] })),
}));
vi.mock("../api/git", () => ({ git: { commitFiles, commitLines } }));
vi.mock("./DiffEditor", () => ({
    DiffEditor: ({ path, baseRev, headRev }: { path: string; baseRev: string; headRev: string }) => (
        <div>{`diff of ${path} from ${baseRev} to ${headRev}`}</div>
    ),
}));

import { CommitReview } from "./CommitReview";

function deferred<T>() {
    let resolve!: (value: T) => void;
    const promise = new Promise<T>((res) => {
        resolve = res;
    });
    return { promise, resolve };
}

beforeEach(() => {
    vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(800);
    vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(1_000);
});

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

describe("CommitReview", () => {
    it("ignores a stale commit-files result after the revision changes", async () => {
        const oldReq = deferred<string[]>();
        const newReq = deferred<string[]>();
        commitFiles.mockReset().mockReturnValueOnce(oldReq.promise).mockReturnValueOnce(newReq.promise);
        const { rerender } = render(<CommitReview repo="/repo" rev="old" title="old" subtitle="" onOpenFile={() => {}} />);
        rerender(<CommitReview repo="/repo" rev="new" title="new" subtitle="" onOpenFile={() => {}} />);

        await act(async () => newReq.resolve(["new.ts"]));
        expect(screen.getByText("new.ts")).toBeInTheDocument();
        await act(async () => oldReq.resolve(["old.ts"]));
        expect(screen.queryByText("old.ts")).not.toBeInTheDocument();
        expect(screen.getByText("new.ts")).toBeInTheDocument();
    });

    it("lists a commit's files folded, and unfolds one against the commit's parent", async () => {
        commitFiles.mockReset().mockResolvedValue(["src/a.ts", "b.ts"]);
        const user = userEvent.setup();
        render(<CommitReview repo="/repo" rev="abc" title="abc" subtitle="fix things" onOpenFile={() => {}} />);
        expect(screen.getByText("abc")).toBeInTheDocument();
        expect(screen.getByText("fix things")).toBeInTheDocument();
        await screen.findByRole("button", { name: "src/a.ts" });
        expect(screen.queryByText(/diff of/)).toBeNull();

        await user.click(screen.getByRole("button", { name: "Expand src/a.ts" }));
        expect(screen.getByText("diff of src/a.ts from abc~1 to abc")).toBeInTheDocument();
        await user.click(screen.getByRole("button", { name: "Collapse src/a.ts" }));
        expect(screen.queryByText(/diff of/)).toBeNull();
    });

    it("opens a file in the editor from its name without folding it", async () => {
        commitFiles.mockReset().mockResolvedValue(["src/a.ts"]);
        const onOpenFile = vi.fn();
        const user = userEvent.setup();
        render(<CommitReview repo="/repo" rev="abc" title="abc" subtitle="" head={<h2>Custom head</h2>} onOpenFile={onOpenFile} />);
        expect(screen.getByRole("heading", { name: "Custom head" })).toBeInTheDocument();
        expect(screen.queryByText("abc")).toBeNull();
        await user.click(await screen.findByRole("button", { name: "src/a.ts" }));
        expect(onOpenFile).toHaveBeenCalledWith("/repo/src/a.ts");
        expect(screen.queryByText(/diff of/)).toBeNull();
    });

    it("shows a range's files open against its base without asking for the commit's own", () => {
        commitFiles.mockReset();
        const range = { base: "main", files: ["x.ts", "y.ts"] };
        render(<CommitReview repo="/repo" rev="head" title="" subtitle="" range={range} onOpenFile={() => {}} />);
        expect(commitFiles).not.toHaveBeenCalled();
        expect(screen.getByText("diff of x.ts from main to head")).toBeInTheDocument();
        expect(screen.getByText("diff of y.ts from main to head")).toBeInTheDocument();
    });

    it("unfolds and scrolls to the file it is pointed at", async () => {
        commitFiles.mockReset().mockResolvedValue(["a.ts", "b.ts"]);
        const { rerender } = render(<CommitReview repo="/repo" rev="abc" title="" subtitle="" onOpenFile={() => {}} />);
        await screen.findByText("b.ts");
        rerender(<CommitReview repo="/repo" rev="abc" title="" subtitle="" focusPath="b.ts" onOpenFile={() => {}} />);
        expect(screen.getByText("diff of b.ts from abc~1 to abc")).toBeInTheDocument();
        expect(screen.queryByText(/diff of a\.ts/)).toBeNull();
    });
});
