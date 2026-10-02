import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { GitPane } from "./GitPane";
import { gitDiscoveredReposR, gitOverviewR } from "../state/resources.defs";
import { getState, setState } from "../state/store";
import { useGitWorkbench } from "../state/gitWorkbench";
import { DEFAULT_GIT_VIEW } from "../state/types";

const resources = vi.hoisted(() => ({
    overview: {
        status: "error",
        data: null,
        error: "could not find repository at '/container'",
        refresh: vi.fn(),
    },
    discovered: {
        status: "ok",
        data: [
            { path: "/container/docs", name: "docs", branch: "main", ahead: 0, behind: 0, changes: 2 },
            { path: "/container/web", name: "web", branch: "main", ahead: 1, behind: 0, changes: 0 },
        ],
        refresh: vi.fn(),
    },
    empty: { status: "ok", data: [], refresh: vi.fn() },
    listing: vi.fn(),
}));

vi.mock("../state/resources", async (original) => ({
    ...(await original<typeof import("../state/resources")>()),
    useCachedResourceEnabled: (enabled: boolean, definition: unknown) => {
        if (definition === gitOverviewR) return resources.overview;
        if (definition === gitDiscoveredReposR) {
            resources.listing(enabled);
            return resources.discovered;
        }
        return resources.empty;
    },
}));
vi.mock("./CommitReview", () => ({ CommitReview: () => <div>Review</div> }));
vi.mock("./MergeReview", () => ({ MergeReview: () => <div>Merge review</div> }));

beforeEach(() => {
    resources.listing.mockClear();
    setState({ gitViews: {}, gitModal: null, pickerOpen: false });
    useGitWorkbench.setState({ drafts: {}, operations: {} });
});
afterEach(cleanup);

it("lists the repositories inside a folder that is not one itself", () => {
    render(<GitPane paneId="git-test" cwd="/container" active visible />);

    expect(screen.getByText("2 repositories inside")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /docs/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /web/ })).toBeInTheDocument();
});

it("opens the repository that was clicked", async () => {
    const user = userEvent.setup();
    render(<GitPane paneId="git-test" cwd="/container" active visible />);

    await user.click(screen.getByRole("button", { name: /docs/ }));

    expect(getState().gitViews["git-test"].repo).toBe("/container/docs");
});

it("shows the chosen repository with a way back to the list", async () => {
    const user = userEvent.setup();
    setState({ gitViews: { "git-test": { ...DEFAULT_GIT_VIEW, repo: "/container/docs" } } });
    render(<GitPane paneId="git-test" cwd="/container" active visible />);

    expect(screen.queryByText("2 repositories inside")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /docs/ }));

    expect(getState().gitViews["git-test"].repo).toBeNull();
});

it("keeps the list live while it is on screen beside the pane being worked in", () => {
    render(<GitPane paneId="git-test" cwd="/container" active={false} visible />);

    expect(resources.listing).toHaveBeenLastCalledWith(true);
});

it("stops keeping it live once it is off screen", () => {
    render(<GitPane paneId="git-test" cwd="/container" active={false} visible={false} />);

    expect(resources.listing).not.toHaveBeenCalledWith(true);
});

it("marks a lone repository that is behind or has no commits yet", () => {
    const discovered = resources.discovered;
    resources.discovered = {
        ...discovered,
        data: [{ path: "/container/empty", name: "empty", branch: "", ahead: 0, behind: 2, changes: 0 }],
    };
    try {
        render(<GitPane paneId="git-test" cwd="/container" active visible />);
        const repo = screen.getByRole("button", { name: /empty/ });
        expect(screen.getByText("1 repository inside")).toBeInTheDocument();
        expect(repo).toHaveTextContent("no commits");
        expect(repo).toHaveTextContent("2↓");
        expect(repo).not.toHaveTextContent("↑");
    } finally {
        resources.discovered = discovered;
    }
});

it("says why a folder with no repository in it could not be read", () => {
    const discovered = resources.discovered;
    try {
        resources.discovered = { ...discovered, status: "error", data: undefined as never, error: "permission denied" } as never;
        const { rerender } = render(<GitPane paneId="git-test" cwd="/container" active visible />);
        expect(screen.getByText("permission denied")).toBeInTheDocument();

        resources.discovered = { ...discovered, data: [] };
        rerender(<GitPane paneId="git-test" cwd="/container" active visible />);
        expect(screen.getByText("No repository at /container, and none in the folders directly inside it.")).toBeInTheDocument();

        resources.discovered = { ...discovered, status: "loading", data: undefined as never };
        rerender(<GitPane paneId="git-test" cwd="/container" active visible />);
        expect(screen.getByRole("status")).toBeInTheDocument();
        expect(screen.queryByText("Not a repository")).toBeNull();
    } finally {
        resources.discovered = discovered;
    }
});
