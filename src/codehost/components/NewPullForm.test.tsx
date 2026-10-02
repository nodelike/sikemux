import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { GitCompare } from "../../api/git";

const api = vi.hoisted(() => ({ branches: vi.fn(), createPull: vi.fn() }));
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
    api.createPull.mockReset().mockResolvedValue({ number: 40 });
    localGit.compare.mockReset().mockResolvedValue(comparison());
    localGit.push.mockReset().mockResolvedValue(undefined);
});

afterEach(cleanup);

describe("NewPullForm", () => {
    it("compares the branch against the usual base and names the pull request after its only commit", async () => {
        const { title } = form();
        await waitFor(() => expect(title().value).toBe("feat: the run page"));
        expect(localGit.compare).toHaveBeenCalledWith("/repo", "main", "feat/x");
        expect(within(right()).getByText("1 commit · 2 files")).toBeInTheDocument();
        expect(within(right()).getByRole("heading", { name: "feat: the run page" })).toBeInTheDocument();
        expect(within(right()).getByTestId("commit-review")).toHaveTextContent("base000..feat/x src/run.ts README.md");
    });

    it("keeps a title already typed, and leaves it empty when there are several commits", async () => {
        localGit.compare.mockResolvedValue(comparison({ commits: [commitOf("a".repeat(14), "one"), commitOf("b".repeat(14), "two")] }));
        const { title } = form();
        await waitFor(() => expect(within(right()).getByText("2 commits · 2 files")).toBeInTheDocument());
        expect(title().value).toBe("");
        expect(within(right()).getByRole("heading", { name: "New pull request" })).toBeInTheDocument();
    });

    it("opens the pull request, as a draft when asked", async () => {
        const { onCreated, title } = form();
        await waitFor(() => expect(title().value).toBe("feat: the run page"));
        await userEvent.type(screen.getByLabelText("Description"), "Why it changed");
        await userEvent.click(screen.getByRole("checkbox", { name: "Draft" }));
        await userEvent.click(screen.getByRole("button", { name: "Open as a draft" }));
        expect(api.createPull).toHaveBeenCalledWith(repo, {
            title: "feat: the run page",
            head: "feat/x",
            base: "main",
            body: "Why it changed",
            draft: true,
        });
        await waitFor(() => expect(onCreated).toHaveBeenCalledWith(40));
        expect(toasts()).toContain("success: Opened #40");
    });

    it("opens it from the keyboard, and says why it could not", async () => {
        api.createPull.mockRejectedValue({ category: "http", message: "http 422: a pull request already exists" });
        const { onCreated, title } = form();
        await waitFor(() => expect(title().value).toBe("feat: the run page"));
        fireEvent.keyDown(title(), { key: "Enter", metaKey: true });
        await waitFor(() => expect(toasts()).toContain("error: Could not open the pull request: http 422: a pull request already exists"));
        expect(onCreated).not.toHaveBeenCalled();
        expect(openButton()).toBeEnabled();
    });

    it("does not open one without a title", async () => {
        localGit.compare.mockResolvedValue(comparison({ commits: [] }));
        const { title } = form();
        await waitFor(() => expect(localGit.compare).toHaveBeenCalled());
        expect(openButton()).toBeDisabled();
        fireEvent.keyDown(title(), { key: "Enter", ctrlKey: true });
        expect(api.createPull).not.toHaveBeenCalled();
        await userEvent.type(title(), "   ");
        expect(openButton()).toBeDisabled();
    });

    it("lets go of the title field on Escape", async () => {
        const { title } = form();
        title().focus();
        fireEvent.keyDown(title(), { key: "Escape" });
        expect(document.activeElement).not.toBe(title());
    });

    it("starts from no branch when the project is on a usual base", async () => {
        form({ head: "main" });
        await waitFor(() => expect(api.branches).toHaveBeenCalled());
        expect(await within(right()).findByText("Choose the branch to open a pull request from.")).toBeInTheDocument();
        expect(screen.getByRole("button", { name: "The branch with the changes" })).toHaveTextContent("Choose a branch");
        expect(localGit.compare).not.toHaveBeenCalled();
    });

    it("compares again against a base picked by hand", async () => {
        form();
        await waitFor(() => expect(localGit.compare).toHaveBeenCalledWith("/repo", "main", "feat/x"));
        await userEvent.click(screen.getByRole("button", { name: "The branch the changes land in" }));
        await userEvent.click(await screen.findByRole("option", { name: "develop" }));
        await waitFor(() => expect(localGit.compare).toHaveBeenLastCalledWith("/repo", "develop", "feat/x"));
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

    it("says why a push failed", async () => {
        api.branches.mockResolvedValue(["main"]);
        localGit.push.mockRejectedValue(new Error("rejected"));
        form();
        await userEvent.click(await screen.findByRole("button", { name: "Push" }));
        await waitFor(() => expect(toasts()).toContain("error: Could not push feat/x: rejected"));
        expect(screen.getByRole("button", { name: "Push" })).toBeEnabled();
    });

    it("says a branch missing from the checkout needs fetching", async () => {
        localGit.compare.mockRejectedValue(new Error("unknown revision"));
        form();
        expect(await within(right()).findByText("feat/x is not in this checkout. Fetch it to see what it changes.")).toBeInTheDocument();
    });

    it("says when the branch has nothing new", async () => {
        localGit.compare.mockResolvedValue(comparison({ files: [], commits: [] }));
        form();
        expect(await within(right()).findByText("feat/x has nothing that main does not.")).toBeInTheDocument();
    });

    it("cannot show the changes of a repository that is not checked out here", async () => {
        form({ cwd: null });
        expect(await within(right()).findByText("This repository is not checked out here, so its changes cannot be shown.")).toBeInTheDocument();
        expect(localGit.compare).not.toHaveBeenCalled();
    });

    it("shows one file, then one commit, then all the changes again", async () => {
        form();
        const left = document.querySelector(".git-left") as HTMLElement;
        await userEvent.click(await within(left).findByRole("button", { name: /README\.md/ }));
        expect(within(right()).getByTestId("commit-review")).toHaveTextContent("@README.md");
        await userEvent.click(screen.getByRole("button", { name: /^Commits/ }));
        await userEvent.click(await screen.findByRole("button", { name: /feat: the run page/ }));
        expect(within(right()).getByTestId("commit-review")).toHaveTextContent("ccccccc3333333");
        await userEvent.click(screen.getByRole("button", { name: "All changes" }));
        expect(within(right()).getByTestId("commit-review")).toHaveTextContent("base000..feat/x");
    });

    it("has no draft box on a host without drafts, and cancels", async () => {
        const plain = { ...host, capabilities: { ...host.capabilities, pulls: { ...host.capabilities.pulls, draft: false } } };
        const { onCancel } = form({ on: plain });
        expect(screen.queryByRole("checkbox", { name: "Draft" })).toBeNull();
        await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
        expect(onCancel).toHaveBeenCalled();
    });
});
