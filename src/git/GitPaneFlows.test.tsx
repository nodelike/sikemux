import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
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
it("offers to stash from the toolbar menu when there are no stashes yet", async () => {
    const user = userEvent.setup();
    render(<GitPane paneId="git-test" cwd="/repo" active visible />);
    await user.click(screen.getByRole("button", { name: "Remotes, stashes and more" }));
    expect(screen.getByRole("menuitem", { name: "No stashes" })).toBeDisabled();
    await user.click(screen.getByRole("menuitem", { name: /Stash changes/ }));
    expect(getState().gitModal).toMatchObject({ kind: "menu", title: "Stash" });
});

it("switches between Changes and Branches from the rail, with History folded under the changes", async () => {
    const user = userEvent.setup();
    render(<GitPane paneId="git-test" cwd="/repo" active visible />);
    const rail = screen.getByRole("navigation", { name: "Git" });
    const changes = within(rail).getByRole("button", { name: "Changes (1)" });
    expect(changes).toHaveAttribute("aria-current", "page");

    const history = screen.getByRole("button", { name: /History/ });
    expect(history).toHaveAttribute("aria-expanded", "false");

    await user.click(history);
    expect(getState().gitViews["git-test"]).toMatchObject({ historyOpen: true, panel: "commits" });
    expect(changes).toHaveAttribute("aria-current", "page");

    await user.click(screen.getByRole("button", { name: /History/ }));
    expect(getState().gitViews["git-test"]).toMatchObject({ historyOpen: false, panel: "files" });

    await user.click(within(rail).getByRole("button", { name: "Branches (2)" }));
    expect(getState().gitViews["git-test"].panel).toBe("branches");
    expect(screen.getByRole("button", { name: /New branch/ })).toBeInTheDocument();
});

it("does not consume text or Tab intended for controls outside the Git pane", () => {
    render(
        <>
            <input aria-label="Rail search" />
            <GitPane paneId="git-test" cwd="/repo" active visible />
        </>,
    );
    const search = screen.getByRole("textbox", { name: "Rail search" });
    search.focus();
    expect(fireEvent.keyDown(search, { key: "s" })).toBe(true);
    expect(fireEvent.keyDown(search, { key: "Tab" })).toBe(true);
    expect(getState().gitModal).toBeNull();
});

it("takes keyboard focus when it becomes the active pane", () => {
    const { rerender } = render(<GitPane paneId="git-test" cwd="/repo" active={false} visible={false} />);
    document.body.focus();
    rerender(<GitPane paneId="git-test" cwd="/repo" active visible />);
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "2" });
    expect(getState().gitViews["git-test"].panel).toBe("branches");
});

it("reuses the diff preview across repeated warm switches", async () => {
    const { rerender } = render(<GitPane paneId="git-test" cwd="/repo" active visible />);
    await screen.findByText("Merge review");
    resources.reviewRender.mockClear();
    const row = screen.getByText("file.ts");
    for (let i = 0; i < 10; i++) {
        rerender(<GitPane paneId="git-test" cwd="/repo" active={false} visible={false} />);
        rerender(<GitPane paneId="git-test" cwd="/repo" active visible />);
        expect(screen.getByText("file.ts")).toBe(row);
    }
    expect(resources.reviewRender).not.toHaveBeenCalled();
});

it("keeps the repository picker up while a folder that is not a repository refetches", () => {
    const overview = resources.overview;
    try {
        resources.overview = { status: "error", error: "could not find repository at '/work'", refresh: vi.fn() } as never;
        const { rerender } = render(<GitPane paneId="git-test" cwd="/work" active visible />);
        expect(screen.getByText("Not a repository")).toBeInTheDocument();
        resources.overview = { status: "loading", refresh: vi.fn() } as never;
        rerender(<GitPane paneId="git-test" cwd="/work" active visible />);
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
            <GitPane paneId="git-test" cwd="/repo" active={active} visible={active} />
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

it("stages and unstages one file from the buttons on its row", async () => {
    const { git } = await import("../api/git");
    const stage = vi.spyOn(git, "stage").mockResolvedValue(undefined as never);
    const unstage = vi.spyOn(git, "unstage").mockResolvedValue(undefined as never);
    const files = resources.overview.data.status.files;
    resources.overview.data.status.files = [{ path: "file.ts", index: "M", worktree: "M" }];
    resources.overview.refresh.mockResolvedValue(undefined);
    resources.empty.refresh.mockResolvedValue(undefined);
    try {
        const user = userEvent.setup();
        render(<GitPane paneId="git-test" cwd="/repo" active visible />);
        await user.click(screen.getByRole("button", { name: "Stage file.ts" }));
        expect(stage).toHaveBeenCalledWith(expect.any(String), "file.ts");
        await user.click(screen.getByRole("button", { name: "Unstage file.ts" }));
        expect(unstage).toHaveBeenCalledWith(expect.any(String), "file.ts");
    } finally {
        resources.overview.data.status.files = files;
        resources.overview.refresh.mockReset();
        resources.empty.refresh.mockReset();
        stage.mockRestore();
        unstage.mockRestore();
    }
});

it("resizes the lists against the review from the divider, and double-click puts it back", () => {
    const offsetWidth = vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(300);
    const clientWidth = vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(1000);
    try {
        render(<GitPane paneId="git-test" cwd="/repo" active visible />);
        const divider = screen.getByRole("separator", { name: "Resize the lists and the review" });

        fireEvent.keyDown(divider, { key: "ArrowRight" });
        expect(getState().gitViews["git-test"].leftWidth).toBe(316);

        fireEvent.keyDown(divider, { key: "ArrowLeft", shiftKey: true });
        expect(getState().gitViews["git-test"].leftWidth).toBe(260);

        offsetWidth.mockReturnValue(620);
        fireEvent.keyDown(divider, { key: "ArrowRight", shiftKey: true });
        expect(getState().gitViews["git-test"].leftWidth).toBe(640);

        fireEvent.doubleClick(divider);
        expect(getState().gitViews["git-test"].leftWidth).toBeNull();
    } finally {
        offsetWidth.mockRestore();
        clientWidth.mockRestore();
    }
});

it("discards every unstaged change after asking, and says which files are new", async () => {
    const discardFiles = vi.spyOn((await import("../api/git")).git, "discardFiles").mockResolvedValue(undefined as never);
    const files = resources.overview.data.status.files;
    resources.overview.data.status.files = [
        { path: "file.ts", index: " ", worktree: "M" },
        { path: "new.ts", index: "?", worktree: "?" },
        { path: "kept.ts", index: "M", worktree: " " },
    ];
    resources.overview.refresh.mockResolvedValue(undefined);
    resources.empty.refresh.mockResolvedValue(undefined);
    try {
        const user = userEvent.setup();
        render(<GitPane paneId="git-test" cwd="/repo" active visible />);
        await user.click(screen.getByRole("button", { name: "Discard all" }));
        const modal = getState().gitModal;
        expect(modal).toMatchObject({ kind: "confirm", title: "Discard unstaged changes in 2 files?", destructive: true });
        expect(modal && "body" in modal ? modal.body : "").toContain("1 new file is deleted");
        expect(discardFiles).not.toHaveBeenCalled();
        await act(async () => {
            if (modal?.kind === "confirm") await modal.onConfirm();
        });
        expect(discardFiles).toHaveBeenCalledWith(expect.any(String), ["file.ts", "new.ts"], "unstaged");
    } finally {
        resources.overview.data.status.files = files;
        resources.overview.refresh.mockReset();
        resources.empty.refresh.mockReset();
        discardFiles.mockRestore();
    }
});

it("grows the open history when its handle moves up", async () => {
    const offsetHeight = vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(200);
    const clientHeight = vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(900);
    try {
        render(<GitPane paneId="git-test" cwd="/repo" active visible />);
        await userEvent.setup().click(screen.getByRole("button", { name: /History/ }));

        fireEvent.keyDown(screen.getByRole("separator", { name: "Resize the history" }), { key: "ArrowUp" });
        expect(getState().gitViews["git-test"].historyHeight).toBe(216);
    } finally {
        offsetHeight.mockRestore();
        clientHeight.mockRestore();
    }
});
