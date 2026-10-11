import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { GitCompare } from "../../api/git";

const api = vi.hoisted(() => ({ branches: vi.fn() }));
const localGit = vi.hoisted(() => ({ compare: vi.fn(), push: vi.fn() }));
vi.mock("../../api/git", async (importOriginal) => ({ ...(await importOriginal<typeof import("../../api/git")>()), git: localGit }));
vi.mock("../../git/CommitReview", () => ({
    CommitReview: ({ rev, range, focusPath }: { rev: string; range?: { base: string; files: string[] }; focusPath?: string | null }) => (
        <div data-testid="commit-review">
            {range ? `${range.base}..${rev} ${range.files.join(" ")}` : rev}
            {focusPath ? ` @${focusPath}` : ""}
        </div>
    ),
}));

import { invalidate } from "../../plugin-api/resources";
import { setState } from "../../state/store";
import { useToasts } from "../../state/toast";
import type { CodeHost } from "../registry";
import { InHost, registerTestHost, TEST_HOST } from "../testHost";
import { NewPullForm } from "./NewPullForm";

const host = registerTestHost(api);
const repo = { provider: TEST_HOST, owner: "nodelike", name: "sikemux" };

const commitOf = (hash: string, subject: string) => ({
    hash: hash.slice(0, 7),
    full_hash: hash,
    parents: [],
    author: "me",
    author_email: "me@example.test",
    date: "now",
    subject,
    refs: [],
    unpushed: false,
});

const comparison = (extra: Partial<GitCompare> = {}): GitCompare =>
    ({
        merge_base: "base000",
        files: [
            { path: "src/run.ts", status: "M" },
            { path: "README.md", status: "A" },
        ],
        commits: [commitOf("ccccccc3333333", "feat: the run page")],
        ...extra,
    }) as GitCompare;

function form({ head = "feat/x" as string | null, cwd = "/repo" as string | null, on = host as CodeHost } = {}) {
    const onCreated = vi.fn();
    const onCancel = vi.fn();
    render(
        <InHost host={on}>
            <NewPullForm paneId="p-new" repo={repo} cwd={cwd} head={head} active onCreated={onCreated} onCancel={onCancel} />
        </InHost>,
    );
    return { onCreated, onCancel, title: () => screen.getByLabelText("Title") as HTMLInputElement };
}

const openButton = () => screen.getByRole("button", { name: /^Open (pull request|as a draft)|Opening…/ });
const right = () => document.querySelector(".git-right") as HTMLElement;
const toasts = () => useToasts.getState().toasts.map((toast) => `${toast.kind}: ${toast.text}`);

beforeEach(() => {
    invalidate(() => true);
    setState({ gitViews: {} });
    useToasts.setState({ toasts: [] });
    api.branches.mockReset().mockResolvedValue(["main", "develop", "feat/x"]);
    localGit.compare.mockReset().mockResolvedValue(comparison());
    localGit.push.mockReset().mockResolvedValue(undefined);
});

afterEach(cleanup);

describe("NewPullForm", () => {
    it("keeps a title already typed, and leaves it empty when there are several commits", async () => {
        localGit.compare.mockResolvedValue(comparison({ commits: [commitOf("a".repeat(14), "one"), commitOf("b".repeat(14), "two")] }));
        const { title } = form();
        await waitFor(() => expect(within(right()).getByText("2 commits · 2 files")).toBeInTheDocument());
        expect(title().value).toBe("");
        expect(within(right()).getByRole("heading", { name: "New pull request" })).toBeInTheDocument();
    });

    it("starts from no branch when the project is on a usual base", async () => {
        form({ head: "main" });
        await waitFor(() => expect(api.branches).toHaveBeenCalled());
        expect(await within(right()).findByText("Choose the branch to open a pull request from.")).toBeInTheDocument();
        expect(screen.getByRole("button", { name: "The branch with the changes" })).toHaveTextContent("Choose a branch");
        expect(localGit.compare).not.toHaveBeenCalled();
    });

    it("offers to push a branch the host does not have yet, then compares again", async () => {
        api.branches.mockResolvedValueOnce(["main"]).mockResolvedValue(["main", "feat/x"]);
        form();
        expect(await screen.findByText("Not on Test host yet")).toBeInTheDocument();
        await waitFor(() => expect(localGit.compare).toHaveBeenCalledTimes(1));
        expect(openButton()).toBeDisabled();
        await userEvent.click(screen.getByRole("button", { name: "Push" }));
        expect(localGit.push).toHaveBeenCalledWith("/repo");
        await waitFor(() => expect(toasts()).toContain("success: Pushed feat/x"));
        await waitFor(() => expect(screen.queryByText("Not on Test host yet")).toBeNull());
        await waitFor(() => expect(localGit.compare).toHaveBeenCalledTimes(2));
    });

    it("cannot show the changes of a repository that is not checked out here", async () => {
        form({ cwd: null });
        expect(await within(right()).findByText("This repository is not checked out here, so its changes cannot be shown.")).toBeInTheDocument();
        expect(localGit.compare).not.toHaveBeenCalled();
    });
});
