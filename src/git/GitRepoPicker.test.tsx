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
