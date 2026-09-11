import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { GitPane } from "./GitPane";
import { gitOverviewR } from "../state/resources.defs";
import { getState, setState } from "../state/store";
import { useGitWorkbench } from "../state/gitWorkbench";
const resources = vi.hoisted(() => ({
    reviewRender: vi.fn(),
    overview: {
        status: "ok",
        data: {
            status: { branch: "main", upstream: null, ahead: 0, behind: 0, files: [{ path: "file.ts", index: " ", worktree: "M" }] },
            branches: [],
            log: [],
        },
        refresh: vi.fn(),
    },
    empty: { status: "ok", data: [], refresh: vi.fn() },
}));
vi.mock("../state/resources", async (original) => ({
    ...(await original<typeof import("../state/resources")>()),
    useCachedResourceEnabled: (_enabled: boolean, definition: unknown) => (definition === gitOverviewR ? resources.overview : resources.empty),
}));
vi.mock("./CommitReview", () => ({ CommitReview: () => <div>Review</div> }));
vi.mock("./MergeReview", () => ({
    MergeReview: () => {
        resources.reviewRender();
        return <div>Merge review</div>;
    },
}));
beforeEach(() => {
    setState({ gitViews: {}, gitModal: null, pickerOpen: false });
    useGitWorkbench.setState({ drafts: {}, operations: {} });
});
afterEach(cleanup);
it("keeps the empty Stashes panel open and offers a next action", async () => {
    const user = userEvent.setup();
    render(<GitPane paneId="git-test" cwd="/repo" active />);
    await user.click(screen.getByRole("button", { name: "Stashes" }));
    expect(getState().gitViews["git-test"].panel).toBe("stashes");
    expect(screen.getByText("No stashed changes.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Stash working changes" })).toBeEnabled();
});
it("does not consume text or Tab intended for controls outside the Git pane", () => {
    render(
        <>
            <input aria-label="Rail search" />
            <GitPane paneId="git-test" cwd="/repo" active />
        </>,
    );
    const search = screen.getByRole("textbox", { name: "Rail search" });
    search.focus();
    expect(fireEvent.keyDown(search, { key: "s" })).toBe(true);
    expect(fireEvent.keyDown(search, { key: "Tab" })).toBe(true);
    expect(getState().gitModal).toBeNull();
});

it("reuses the diff preview across repeated warm switches", async () => {
    const { rerender } = render(<GitPane paneId="git-test" cwd="/repo" active />);
    await screen.findByText("Merge review");
    resources.reviewRender.mockClear();
    const row = screen.getByText("file.ts");
    for (let i = 0; i < 10; i++) {
        rerender(<GitPane paneId="git-test" cwd="/repo" active={false} />);
        rerender(<GitPane paneId="git-test" cwd="/repo" active />);
        expect(screen.getByText("file.ts")).toBe(row);
    }
    expect(resources.reviewRender).not.toHaveBeenCalled();
});
