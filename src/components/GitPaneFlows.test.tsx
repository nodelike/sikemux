import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { GitPane } from "./GitPane";
import { gitOverviewR } from "../state/resources.defs";
import { getState, setState } from "../state/store";
import { useGitWorkbench } from "../state/gitWorkbench";
import { useStageMotion } from "../state/nativeViews";
const resources = vi.hoisted(() => ({
    reviewRender: vi.fn(),
    overviewEnabled: false,
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
    useCachedResourceEnabled: (enabled: boolean, definition: unknown) => {
        if (definition !== gitOverviewR) return resources.empty;
        resources.overviewEnabled = enabled;
        return resources.overview;
    },
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

it("takes keyboard focus when it becomes the active pane", () => {
    const { rerender } = render(<GitPane paneId="git-test" cwd="/repo" active={false} />);
    document.body.focus();
    rerender(<GitPane paneId="git-test" cwd="/repo" active />);
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "3" });
    expect(getState().gitViews["git-test"].panel).toBe("branches");
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

it("keeps the repository picker up while a folder that is not a repository refetches", () => {
    const overview = resources.overview;
    try {
        resources.overview = { status: "error", error: "could not find repository at '/work'", refresh: vi.fn() } as never;
        const { rerender } = render(<GitPane paneId="git-test" cwd="/work" active />);
        expect(screen.getByText("Not a repository")).toBeInTheDocument();
        resources.overview = { status: "loading", refresh: vi.fn() } as never;
        rerender(<GitPane paneId="git-test" cwd="/work" active />);
        expect(screen.getByText("Not a repository")).toBeInTheDocument();
        expect(screen.queryByRole("button", { name: "Stashes" })).toBeNull();
    } finally {
        resources.overview = overview;
    }
});

function StageSliding() {
    useStageMotion(true);
    return null;
}

const nextFrame = () => act(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())));

it("waits for the stage to stop sliding before it refreshes", async () => {
    const stage = (sliding: boolean, active: boolean) => (
        <>
            {sliding && <StageSliding />}
            <GitPane paneId="git-test" cwd="/repo" active={active} />
        </>
    );
    const { rerender } = render(stage(false, false));
    rerender(stage(true, true));
    await nextFrame();
    await nextFrame();
    expect(resources.overviewEnabled).toBe(false);
    rerender(stage(false, true));
    await nextFrame();
    expect(resources.overviewEnabled).toBe(true);
});
