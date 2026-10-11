import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { git, type GitBranch, type GitCommit, type GitFile, type GitRemote, type GitRemoteBranch, type GitStash } from "../api/git";
import { gitOverviewR, gitRemoteBranchesR, gitRemotesR, gitStashesR } from "../state/resources.defs";
import { getState, setState } from "../state/store";
import { useGitWorkbench } from "../state/gitWorkbench";
import { useToasts } from "../state/toast";
import { GitPane } from "./GitPane";

type Resource<T> = { status: string; data?: T; error?: string; refresh: ReturnType<typeof vi.fn> };

const h = vi.hoisted(() => ({
    overview: null as unknown as Resource<{ status: unknown; branches: unknown[]; log: unknown[] }>,
    remotes: null as unknown as Resource<unknown[]>,
    stashes: null as unknown as Resource<unknown[]>,
    remoteBranches: null as unknown as Resource<unknown[]>,
    empty: null as unknown as Resource<unknown[]>,
    copyText: vi.fn(),
}));

vi.mock("../state/resources", async (original) => ({
    ...(await original<typeof import("../state/resources")>()),
    useCachedResourceEnabled: (_enabled: boolean, definition: unknown) => {
        if (definition === gitOverviewR) return h.overview;
        if (definition === gitRemotesR) return h.remotes;
        if (definition === gitStashesR) return h.stashes;
        if (definition === gitRemoteBranchesR) return h.remoteBranches;
        return h.empty;
    },
}));
vi.mock("../lib/clipboard", () => ({ copyText: h.copyText }));
vi.mock("./CommitReview", () => ({
    CommitReview: ({ rev, head }: { rev: string; head?: ReactNode }) => <section aria-label={`Commit review ${rev}`}>{head}</section>,
}));
vi.mock("./MergeReview", () => ({
    MergeReview: ({ files, fileActions }: { files: GitFile[]; fileActions?: (file: GitFile) => ReactNode }) => (
        <section aria-label="Change review">
            {files.map((file) => (
                <div key={file.path}>{fileActions?.(file)}</div>
            ))}
        </section>
    ),
}));

const REPO = "/repo";
const PANE = "git-test";

const commit = (i: number, extra: Partial<GitCommit> = {}): GitCommit => ({
    hash: `c${i}`,
    full_hash: `full${i}`,
    parents: [`full${i + 1}`],
    author: "Ada",
    author_email: "ada@example.test",
    date: "1h ago",
    subject: `subject ${i}`,
    refs: [],
    unpushed: false,
    ...extra,
});

const files = (): GitFile[] => [
    { path: "src/a.ts", index: "M", worktree: " " },
    { path: "src/b.ts", index: " ", worktree: "M" },
    { path: "new.ts", index: "?", worktree: "?" },
];
const branches = (): GitBranch[] => [
    { name: "main", current: true, upstream: "origin/main" },
    { name: "feature", current: false, upstream: null },
];
const remoteBranch = (name: string, extra: Partial<GitRemoteBranch> = {}): GitRemoteBranch => ({
    name,
    full_ref: `origin/${name}`,
    is_head_pointer: false,
    tracked_by: null,
    subject: null,
    ...extra,
});

const resource = <T,>(data: T): Resource<T> => ({ status: "ok", data, refresh: vi.fn().mockResolvedValue(undefined) });

function setOverview(patch: {
    files?: GitFile[];
    branches?: GitBranch[];
    log?: GitCommit[];
    upstream?: string | null;
    ahead?: number;
    behind?: number;
}) {
    h.overview = resource({
        status: {
            branch: "main",
            upstream: patch.upstream === undefined ? "origin/main" : patch.upstream,
            ahead: patch.ahead ?? 2,
            behind: patch.behind ?? 1,
            files: patch.files ?? files(),
        },
        branches: patch.branches ?? branches(),
        log: patch.log ?? [commit(0, { refs: ["HEAD -> main", "tag: v1", "HEAD"], unpushed: true }), commit(1)],
    });
}

const gitCalls = [
    "stage",
    "unstage",
    "stageAll",
    "unstageAll",
    "stagePaths",
    "discardFiles",
    "stashPush",
    "stashApply",
    "stashPop",
    "stashBranch",
    "stashRename",
    "stashDrop",
    "branchCreate",
    "reset",
    "revert",
    "checkout",
    "merge",
    "mergeSquash",
    "branchRename",
    "branchDelete",
    "checkoutRemoteBranch",
    "setUpstream",
    "deleteRemoteBranch",
    "fetch",
    "remoteAdd",
    "remoteSetUrl",
    "remoteRename",
    "remoteRemove",
    "push",
    "pull",
    "prOpen",
    "commit",
    "aiMessage",
] as const;

type GitMock = ReturnType<typeof vi.fn>;
const api = git as unknown as Record<(typeof gitCalls)[number] | "status", GitMock>;

beforeEach(() => {
    setOverview({});
    h.remotes = resource<GitRemote[]>([{ name: "origin", url: "git@example.test:o/r.git" }]);
    h.stashes = resource<GitStash[]>([{ index: 0, sha: "s0", refname: "stash@{0}", branch: "main", message: "WIP" }]);
    h.remoteBranches = resource<GitRemoteBranch[]>([
        remoteBranch("HEAD", { is_head_pointer: true }),
        remoteBranch("main", { tracked_by: "main" }),
        remoteBranch("topic"),
    ]);
    h.empty = resource<unknown[]>([]);
    h.copyText.mockReset().mockResolvedValue(undefined);
    for (const name of gitCalls) vi.spyOn(git, name).mockResolvedValue("" as never);
    vi.spyOn(git, "status").mockResolvedValue({ branch: "main", upstream: null, ahead: 0, behind: 0, files: files() });
    setState({ gitViews: {}, gitModal: null, pickerOpen: false, gitCmdLogOpen: false });
    useGitWorkbench.setState({ drafts: {}, operations: {}, provider: "hermes", model: "openai/gpt-5.5" });
    useToasts.setState({ toasts: [] });
});

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

const renderPane = () => render(<GitPane paneId={PANE} cwd={REPO} active visible />);
const view = () => getState().gitViews[PANE];
const toasts = () => useToasts.getState().toasts.map((t) => `${t.kind}: ${t.text}`);
const press = (key: string, init: KeyboardEventInit = {}) => fireEvent.keyDown(document.activeElement ?? document.body, { key, ...init });
const flush = () => act(async () => new Promise<void>((resolve) => setTimeout(resolve, 0)));

function modalOf<K extends "menu" | "prompt" | "confirm" | "cheatsheet">(kind: K) {
    const modal = getState().gitModal;
    if (modal?.kind !== kind) throw new Error(`expected a ${kind}, got ${modal?.kind ?? "nothing"}`);
    return modal as Extract<NonNullable<typeof modal>, { kind: K }>;
}

async function choose(label: string) {
    const item = modalOf("menu").items.find((i) => i.label === label);
    if (!item) throw new Error(`no menu item "${label}"`);
    act(() => setState({ gitModal: null }));
    await act(async () => {
        await item.run();
    });
    await flush();
}

async function answer(value: string) {
    const prompt = modalOf("prompt");
    act(() => setState({ gitModal: null }));
    await act(async () => {
        await prompt.onConfirm(value);
    });
    await flush();
}

async function accept() {
    const confirm = modalOf("confirm");
    act(() => setState({ gitModal: null }));
    await act(async () => {
        await confirm.onConfirm();
    });
    await flush();
}

const row = (name: string, nth = 0) => screen.getAllByText(name, { selector: ".git-row-name" })[nth].closest<HTMLElement>(".git-row")!;
const menuItem = (name: RegExp) => screen.getByRole("menuitem", { name });

describe("changes", () => {
    it("stages a range marked with v and extended past the group header", async () => {
        renderPane();
        press("v");
        press("j");
        press("j");
        expect(row("new.ts")).toHaveClass("sel");
        expect(row("b.ts")).toHaveClass("ranged");
        press(" ");
        await flush();
        expect(api.stagePaths).toHaveBeenCalledWith(REPO, ["src/b.ts", "new.ts"]);
    });

    it("drops a marked range on Escape and space then acts on the one selected file", async () => {
        renderPane();
        press("v");
        press("Escape");
        press(" ");
        await flush();
        expect(api.unstage).toHaveBeenCalledWith(REPO, "src/a.ts");
        expect(api.stagePaths).not.toHaveBeenCalled();
    });

    it("stages everything with a while anything is unstaged, and unstages everything once nothing is", async () => {
        const { rerender } = renderPane();
        press("a");
        await flush();
        expect(api.stageAll).toHaveBeenCalledWith(REPO);

        setOverview({ files: [{ path: "src/a.ts", index: "M", worktree: " " }] });
        rerender(<GitPane paneId={PANE} cwd={REPO} active visible />);
        press("a");
        await flush();
        expect(api.unstageAll).toHaveBeenCalledWith(REPO);
    });

    it("discards one file from its row, warning when the file is new", async () => {
        const user = userEvent.setup();
        renderPane();
        const discard = within(row("new.ts")).getByRole("button", { name: "Discard changes" });
        await user.click(discard);
        expect(modalOf("confirm")).toMatchObject({ title: "Discard changes to new.ts?", body: expect.stringContaining("new and will be deleted") });
        await accept();
        expect(api.discardFiles).toHaveBeenCalledWith(REPO, ["new.ts"], "unstaged");

        await user.click(within(row("b.ts")).getByRole("button", { name: "Discard changes" }));
        expect(modalOf("confirm").body).toContain("Unstaged changes to this file will be lost");
    });

    it("offers staged, unstaged and full discards for the selected file", async () => {
        renderPane();
        press("d");
        expect(modalOf("menu").title).toBe("Discard changes — a.ts");
        await choose("unstage changes");
        expect(modalOf("confirm")).toMatchObject({ confirmLabel: "unstage", body: "Unstage 1 file from the index? (worktree preserved)" });
        await accept();
        expect(api.discardFiles).toHaveBeenCalledWith(REPO, ["src/a.ts"], "staged");

        press("d");
        await choose("discard unstaged changes");
        expect(modalOf("confirm").body).toBe("Discard unstaged changes in 1 file? (staged changes preserved)");
    });

    it("discards every file in a marked range, both staged and unstaged", async () => {
        renderPane();
        press("v");
        press("j");
        press("j");
        press("d");
        expect(modalOf("menu").title).toBe("Discard 3 files");
        await choose("discard all changes");
        expect(modalOf("confirm").body).toContain("BOTH staged and unstaged changes in 3 files");
        await accept();
        expect(api.discardFiles).toHaveBeenCalledWith(REPO, ["src/a.ts", "src/b.ts", "new.ts"], "all");
    });

    it("stashes only the staged changes, with no message when none is given", async () => {
        renderPane();
        press("s");
        await choose("stash staged only");
        expect(modalOf("prompt").title).toBe("Stash staged changes");
        await answer("");
        expect(api.stashPush).toHaveBeenCalledWith(REPO, "staged", null);
        expect(toasts()).toContain("success: Stashed staged changes");

        press("s");
        await choose("stash everything (working tree + index + untracked)");
        await answer("keep");
        expect(api.stashPush).toHaveBeenCalledWith(REPO, "all", "keep");
        expect(toasts()).toContain("success: Stashed your changes");
    });

    it("commits the draft with C and clears it once committed", async () => {
        useGitWorkbench.setState({ drafts: { [REPO]: "fix: things" } });
        renderPane();
        press("c");
        expect(screen.getByRole("textbox", { name: "Commit message" })).toHaveFocus();
        act(() => (document.activeElement as HTMLElement).blur());
        act(() => screen.getByText("a.ts", { selector: ".git-row-name" }).closest<HTMLElement>(".git-row")!.focus());
        press("C");
        await waitFor(() => expect(api.commit).toHaveBeenCalledWith(REPO, "fix: things"));
        await waitFor(() => expect(useGitWorkbench.getState().drafts[REPO]).toBe(""));
    });

    it("filters the changed files from /, and Escape clears the filter", async () => {
        const user = userEvent.setup();
        renderPane();
        press("/");
        const filter = screen.getByPlaceholderText("Filter files");
        expect(filter).toHaveFocus();
        await user.type(filter, "zzz");
        expect(screen.getByText("No matches")).toBeInTheDocument();
        expect(screen.getByText('Nothing matches "zzz".')).toBeInTheDocument();

        await user.clear(filter);
        await user.type(filter, "b.ts{Enter}");
        expect(screen.queryByText("new.ts")).toBeNull();
        await waitFor(() => expect(row("b.ts")).toHaveFocus());

        press("Escape");
        expect(screen.getByText("new.ts")).toBeInTheDocument();
        expect(screen.queryByPlaceholderText("Filter files")).toBeNull();
    });

    it("says the tree is clean and reviews the latest commit when nothing changed", () => {
        setOverview({ files: [] });
        renderPane();
        expect(screen.getByText("Nothing to commit")).toBeInTheDocument();
        expect(screen.getByRole("region", { name: "Commit review c0" })).toBeInTheDocument();
    });
});

describe("history", () => {
    it("drives the selected commit from the keyboard", async () => {
        renderPane();
        press("h");
        press("r");
        expect(modalOf("menu").title).toBe("Reset to c0");
        await choose("soft reset");
        expect(modalOf("confirm")).toMatchObject({ destructive: false, body: expect.stringContaining("staged") });
        act(() => setState({ gitModal: null }));

        press("r");
        await choose("mixed reset");
        expect(modalOf("confirm").body).toContain("working tree");
        act(() => setState({ gitModal: null }));

        press("b");
        expect(modalOf("prompt").title).toBe("Branch from c0");
        await answer("   ");
        expect(api.branchCreate).not.toHaveBeenCalled();

        press("v");
        expect(modalOf("confirm").title).toBe("Revert c0?");
        act(() => setState({ gitModal: null }));

        press("Enter");
        expect(modalOf("menu").title).toBe("c0 · subject 0");
        await choose("copy hash");
        expect(h.copyText).toHaveBeenCalledWith("full0");
    });

    it("searches the commits and says when nothing matches", async () => {
        const user = userEvent.setup();
        renderPane();
        await user.click(screen.getByRole("button", { name: "Search commits" }));
        const search = screen.getByRole("textbox", { name: "Search commits" });
        expect(search).toHaveFocus();
        await user.type(search, "nope");
        expect(screen.getByText('Nothing matches "nope".')).toBeInTheDocument();
        await user.clear(search);
        await user.type(search, "subject 1");
        expect(screen.queryByRole("button", { name: "c0 subject 0" })).toBeNull();
        await user.type(search, "{Escape}");
        expect(screen.getByRole("button", { name: "c0 subject 0" })).toBeInTheDocument();
        await waitFor(() => expect(screen.getByRole("button", { name: "c0 subject 0" })).toHaveFocus());
        expect(screen.queryByRole("textbox", { name: "Search commits" })).toBeNull();
    });

    it("focuses the first commit when a search that matched nothing is dropped", async () => {
        const user = userEvent.setup();
        renderPane();
        await user.click(screen.getByRole("button", { name: "Search commits" }));
        await user.type(screen.getByRole("textbox", { name: "Search commits" }), "nope{Escape}");
        await waitFor(() => expect(screen.getByRole("button", { name: "c0 subject 0" })).toHaveFocus());
    });

    it("closes the search on blur only while it is empty", async () => {
        const user = userEvent.setup();
        renderPane();
        press("h");
        press("/");
        const search = screen.getByRole("textbox", { name: "Search commits" });
        act(() => search.blur());
        expect(screen.queryByRole("textbox", { name: "Search commits" })).toBeNull();

        await user.click(screen.getByRole("button", { name: "Search commits" }));
        await user.type(screen.getByRole("textbox", { name: "Search commits" }), "subject 1{Enter}");
        expect(screen.getByRole("textbox", { name: "Search commits" })).toHaveValue("subject 1");
        expect(screen.queryByRole("button", { name: "c0 subject 0" })).toBeNull();
    });
});

describe("branches", () => {
    const openBranches = () => {
        renderPane();
        press("2");
    };

    it("refuses to delete the checked-out branch", () => {
        openBranches();
        press("d");
        const menu = modalOf("menu");
        expect(menu.items.every((item) => item.disabled && item.hint === "(can't delete current branch)")).toBe(true);
    });

    it("opens a remote to its branches and acts on one without a local copy", async () => {
        const user = userEvent.setup();
        openBranches();
        expect(row("origin")).toHaveAttribute("aria-expanded", "true");
        expect(screen.queryByText("HEAD", { selector: ".git-row-name" })).toBeNull();
        expect(within(row("main", 1)).getByText("tracked")).toBeInTheDocument();
        await user.click(row("origin"));
        expect(view().openRemote).toBeNull();
        expect(screen.queryByText("topic")).toBeNull();
        await user.click(row("origin"));
        expect(view().openRemote).toBe("origin");

        await user.click(row("topic"));
        const head = screen.getByRole("region", { name: "Commit review origin/topic" });
        expect(within(head).getByText("no local branch")).toBeInTheDocument();

        await user.click(within(head).getByRole("button", { name: "Check out" }));
        await flush();
        expect(api.checkoutRemoteBranch).toHaveBeenCalledWith(REPO, "origin", "topic", null);
        expect(toasts()).toContain("success: Switched to topic");

        await user.click(within(head).getByRole("button", { name: "Set as upstream" }));
        await flush();
        expect(api.setUpstream).toHaveBeenCalledWith(REPO, "main", "origin/topic");
        expect(toasts()).toContain("success: main now tracks origin/topic");

        await user.click(within(head).getByRole("button", { name: "Merge into main" }));
        expect(modalOf("menu").title).toBe("Merge origin/topic into main");
        await choose("regular merge");
        expect(api.merge).toHaveBeenCalledWith(REPO, "origin/topic");

        await user.click(within(head).getByRole("button", { name: "Delete on origin" }));
        await accept();
        expect(api.deleteRemoteBranch).toHaveBeenCalledWith(REPO, "origin", "topic");
        expect(h.remoteBranches.refresh).toHaveBeenCalled();
    });

    it("checks out the local branch that already tracks a remote one", async () => {
        const user = userEvent.setup();
        openBranches();
        const tracked = row("main", 1);
        await user.click(within(tracked).getByRole("button", { name: "Check out main" }));
        await flush();
        expect(api.checkoutRemoteBranch).toHaveBeenCalledWith(REPO, "origin", "main", "main");
        expect(toasts()).toContain("success: Switched to main");

        await user.click(within(tracked).getByRole("button", { name: "More" }));
        await user.click(menuItem(/Delete on origin/));
        expect(modalOf("confirm").title).toBe("Delete origin/main?");
    });

    it("keeps an open remote open under its new name", async () => {
        const user = userEvent.setup();
        openBranches();
        await user.click(within(row("origin")).getByRole("button", { name: "More" }));
        await user.click(menuItem(/Rename/));
        await answer("upstream");
        expect(api.remoteRename).toHaveBeenCalledWith(REPO, "origin", "upstream");
        expect(view().openRemote).toBe("upstream");
    });

    it("drives the branch list from the keyboard", async () => {
        openBranches();
        press("Enter");
        await flush();
        expect(api.checkout).not.toHaveBeenCalled();

        press("N");
        expect(modalOf("prompt").title).toBe("New branch");
        await answer("fresh");
        expect(api.branchCreate).toHaveBeenCalledWith(REPO, "fresh", undefined);
        expect(toasts()).toContain("success: Created fresh and switched to it");

        press("c");
        expect(modalOf("prompt").suggestions).toEqual([
            { value: "main", hint: "current" },
            { value: "feature", hint: "" },
        ]);
        await answer(" feature ");
        expect(api.checkout).toHaveBeenCalledWith(REPO, "feature");

        press("f");
        await flush();
        expect(api.fetch).toHaveBeenCalledWith(REPO, null);

        press("j");
        press("n");
        expect(modalOf("prompt").title).toBe("New branch from feature");
        act(() => setState({ gitModal: null }));

        press("M");
        expect(modalOf("menu").title).toBe("Merge feature into main");
        await choose("squash merge");
        expect(api.mergeSquash).toHaveBeenCalledWith(REPO, "feature");

        press("R");
        expect(modalOf("prompt").title).toBe("Rename branch · feature");
        act(() => setState({ gitModal: null }));

        press(" ");
        await flush();
        expect(api.checkout).toHaveBeenCalledTimes(2);

        press("j");
        press("f");
        await flush();
        expect(api.fetch).toHaveBeenCalledWith(REPO, "origin");
        press("Enter");
        expect(view().openRemote).toBeNull();
    });

    it("acts on a selected remote branch from the keyboard", async () => {
        openBranches();
        await userEvent.setup().click(row("topic"));
        press("n");
        expect(modalOf("prompt").title).toBe("New branch from origin/topic");
        act(() => setState({ gitModal: null }));

        press("M");
        expect(modalOf("menu").title).toBe("Merge origin/topic into main");
        act(() => setState({ gitModal: null }));

        press("u");
        await flush();
        expect(api.setUpstream).toHaveBeenCalledWith(REPO, "main", "origin/topic");

        press("f");
        await flush();
        expect(api.fetch).toHaveBeenCalledWith(REPO, "origin");

        press("d");
        expect(modalOf("confirm").title).toBe("Delete origin/topic?");
        act(() => setState({ gitModal: null }));

        press("Enter");
        await flush();
        expect(api.checkoutRemoteBranch).toHaveBeenCalledWith(REPO, "origin", "topic", null);
    });

    it("calls an unpublished current branch's push a publish", () => {
        setOverview({ upstream: null, ahead: 0 });
        openBranches();
        const head = screen.getByRole("region", { name: "Commit review main" });
        expect(within(head).getByRole("button", { name: "Publish" })).toBeInTheDocument();
    });
});

describe("toolbar", () => {
    it("shows a failed operation in the toolbar and as an error", async () => {
        api.push.mockRejectedValue(new Error("rejected: non-fast-forward\nhint: pull first"));
        renderPane();
        press("P");
        await flush();
        expect(screen.getByText("✗ rejected: non-fast-forward\nhint: pull first", { normalizer: (s) => s })).toBeInTheDocument();
        expect(toasts()).toContain("error: rejected: non-fast-forward");
    });

    it("ignores keys while another operation holds the repository", async () => {
        useGitWorkbench.setState({ operations: { [REPO]: { label: "pulling…", busy: true, error: null, result: null } } });
        const user = userEvent.setup();
        renderPane();
        expect(document.querySelector(".git-busy-label")).toHaveTextContent("pulling…");
        press("P");
        await user.click(screen.getByRole("button", { name: /^Push\s*2$/ }));
        await flush();
        expect(api.push).not.toHaveBeenCalled();
    });

    it("adds a remote by name and then URL, stopping at a blank answer", async () => {
        const user = userEvent.setup();
        renderPane();
        const more = screen.getByRole("button", { name: "Remotes, stashes and more" });
        await user.click(more);
        await user.click(menuItem(/Add remote/));
        await answer(" ");
        expect(getState().gitModal).toBeNull();

        await user.click(more);
        await user.click(menuItem(/Add remote/));
        await answer("upstream");
        expect(modalOf("prompt").title).toBe("Add remote · upstream");
        await answer("");
        expect(api.remoteAdd).not.toHaveBeenCalled();

        await user.click(more);
        await user.click(menuItem(/Add remote/));
        await answer("upstream");
        await answer("https://example.test/u.git");
        expect(api.remoteAdd).toHaveBeenCalledWith(REPO, "upstream", "https://example.test/u.git");
        expect(toasts()).toContain("success: Added remote upstream");
    });
});

describe("keyboard", () => {
    it("leaves keys alone that carry a modifier or arrive while the picker or another area has the pane", async () => {
        renderPane();
        press("P", { altKey: true });
        press("P", { metaKey: true });
        press("P", { ctrlKey: true });
        act(() => setState({ pickerOpen: true }));
        press("P");
        act(() => setState({ pickerOpen: false, gitViews: { [PANE]: { ...view(), area: "pulls" } } }));
        press("P");
        await flush();
        expect(api.push).not.toHaveBeenCalled();

        act(() => setState({ gitViews: { [PANE]: { ...view(), area: "local" } } }));
        press("P");
        await flush();
        expect(api.push).toHaveBeenCalledOnce();
    });

    it("steps over the group headers when moving through the files", () => {
        renderPane();
        press("k");
        expect(row("a.ts")).toHaveClass("sel");
        press("j");
        expect(row("b.ts")).toHaveClass("sel");
        press("ArrowDown");
        press("j");
        expect(row("new.ts")).toHaveClass("sel");
        press("k");
        press("ArrowUp");
        expect(row("a.ts")).toHaveClass("sel");
    });

    it("clears a branch filter with Escape and returns to it with /", async () => {
        const user = userEvent.setup();
        renderPane();
        press("2");
        press("h");
        expect(view()).toMatchObject({ historyOpen: true, panel: "branches" });

        const filter = screen.getByPlaceholderText("Filter branches");
        await user.type(filter, "feat{Enter}");
        await waitFor(() => expect(row("feature")).toHaveFocus());
        press("Escape");
        expect(filter).toHaveValue("");
        expect(row("main")).toBeInTheDocument();

        press("/");
        expect(filter).toHaveFocus();
        await user.type(filter, "x{Escape}");
        expect(filter).toHaveValue("");

        await user.click(screen.getByRole("button", { name: "Changes (1)" }));
        expect(view()).toMatchObject({ area: "local", panel: "files" });
    });
});

describe("details", () => {
    it("cuts a long failure short in the toolbar", async () => {
        api.pull.mockRejectedValue(new Error("x".repeat(120)));
        renderPane();
        press("p");
        await flush();
        expect(document.querySelector(".git-tb-busy.error")).toHaveTextContent(`✗ ${"x".repeat(80)}…`);
    });

    it("closes the history with h and leaves the file list where it was", async () => {
        const user = userEvent.setup();
        renderPane();
        await user.click(row("b.ts"));
        press("h");
        expect(view()).toMatchObject({ historyOpen: true, panel: "commits" });
        press("h");
        expect(view()).toMatchObject({ historyOpen: false, panel: "files" });
        expect(row("b.ts")).toHaveClass("sel");
    });

    it("stages nothing when a marked range holds only staged files", async () => {
        renderPane();
        press("v");
        expect(row("a.ts")).toHaveClass("ranged");
        press(" ");
        await flush();
        expect(api.stagePaths).not.toHaveBeenCalled();
        expect(api.unstage).not.toHaveBeenCalled();
        expect(row("a.ts")).not.toHaveClass("ranged");

        press("v");
        press("v");
        expect(row("a.ts")).not.toHaveClass("ranged");
    });

    it("lets Escape through when there is nothing to clear", () => {
        renderPane();
        expect(press("Escape")).toBe(true);
        expect(press("x")).toBe(true);
    });

    it("does nothing with space when there are no changed files", async () => {
        setOverview({ files: [] });
        renderPane();
        press(" ");
        press("d");
        await flush();
        expect(api.stage).not.toHaveBeenCalled();
        expect(getState().gitModal).toBeNull();
    });
});
