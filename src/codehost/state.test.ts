import { describe, expect, it } from "vitest";
import { emit } from "../state/bus";
import {
    closeRun,
    compose,
    filterBy,
    forgetAccount,
    hostSettings,
    isSection,
    leaveRun,
    needsRepo,
    openRunFrom,
    refOf,
    setListState,
    resetView,
    sameRepo,
    setFollowBranch,
    setProjectAccount,
    setProjectRepo,
    showRun,
    togglePinned,
    updateView,
    viewOf,
    type HostSettings,
} from "./state";

const HOST = "test.host";
const settings = hostSettings(HOST);

describe("hostSettings", () => {
    it("falls back to usable settings whatever was saved", () => {
        settings.update(
            () =>
                ({
                    pinned: ["nodelike/sikemux", "not a repo", 7, "nodelike/sikemux"],
                    repoByProject: { "/repo": "owner/name", "/bad": 3, "/alsobad": "owner" },
                    accountByProject: { "/repo": "work-id", "/bad": 4, "/empty": "" },
                }) as unknown as HostSettings,
        );
        expect(settings.get()).toEqual({
            pinned: ["nodelike/sikemux"],
            repoByProject: { "/repo": "owner/name" },
            accountByProject: { "/repo": "work-id" },
            followBranch: true,
        });
    });

    it("forgets an account in every project that picked it once it signs out", () => {
        setProjectAccount(HOST, "/work", "work-id");
        setProjectAccount(HOST, "/also-work", "work-id");
        setProjectAccount(HOST, "/home", "home-id");
        forgetAccount(HOST, "work-id");
        expect(settings.get().accountByProject).toEqual({ "/home": "home-id" });
        setProjectAccount(HOST, "/home", null);
        expect(settings.get().accountByProject).toEqual({});
    });

    it("keeps following the branch unless it was turned off on purpose", () => {
        setFollowBranch(HOST, false);
        expect(settings.get().followBranch).toBe(false);
        settings.update(() => ({}) as unknown as HostSettings);
        expect(settings.get().followBranch).toBe(true);
    });

    it("pins and unpins the same repository with one call", () => {
        settings.update(() => ({}) as unknown as HostSettings);
        togglePinned(HOST, "a/b");
        expect(settings.get().pinned).toEqual(["a/b"]);
        togglePinned(HOST, "a/b");
        expect(settings.get().pinned).toEqual([]);
    });

    it("forgets a project's repository when it is cleared", () => {
        settings.update(() => ({}) as unknown as HostSettings);
        setProjectRepo(HOST, "/work", "a/b");
        expect(settings.get().repoByProject).toEqual({ "/work": "a/b" });
        setProjectRepo(HOST, "/work", null);
        expect(settings.get().repoByProject).toEqual({});
    });

    it("keeps each host's settings apart", () => {
        togglePinned(HOST, "a/b");
        expect(hostSettings("other.host").get().pinned).toEqual([]);
    });
});

describe("refOf", () => {
    it("reads owner and repo out of a slug, on the host it was asked for", () => {
        expect(refOf(HOST, "nodelike/sikemux")).toEqual({ provider: HOST, owner: "nodelike", name: "sikemux" });
    });

    it("refuses anything that is not exactly one slug", () => {
        for (const bad of ["", "nodelike", "a/b/c", "/b", "a/"]) {
            expect(refOf(HOST, bad)).toBeNull();
        }
    });
});

describe("the view of one pane", () => {
    it("clears the open run and goes back to the first page when a filter changes", () => {
        updateView("pane-2", { page: 4 });
        showRun("pane-2", "99");
        expect(viewOf("pane-2").run).toBe("99");

        filterBy("pane-2", { statusFilter: "failure" });
        expect(viewOf("pane-2")).toMatchObject({ statusFilter: "failure", page: 1, run: null, job: null });
    });

    it("closes a run without touching the filters", () => {
        updateView("pane-3", { statusFilter: "failure" });
        showRun("pane-3", "7");
        updateView("pane-3", { job: "3" });
        closeRun("pane-3");
        expect(viewOf("pane-3")).toMatchObject({ statusFilter: "failure", run: null, job: null });
    });

    it("starts over when told the repository changed", () => {
        updateView("pane-4", { statusFilter: "failure", page: 3, workflowId: "12" });
        compose("pane-4", "pull");
        resetView("pane-4");
        expect(viewOf("pane-4")).toMatchObject({ statusFilter: "all", page: 1, workflowId: null, composing: null });
    });
});

describe("a run opened from a pull request's check", () => {
    it("asks for the failed job and goes back to the pull request", () => {
        openRunFrom("pane-5", "412", 31);
        expect(viewOf("pane-5")).toMatchObject({ run: "412", item: null, runFrom: 31, pickFailed: true });
        leaveRun("pane-5");
        expect(viewOf("pane-5")).toMatchObject({ run: null, item: 31, runFrom: null });
    });

    it("goes back to the runs list when it was opened from there", () => {
        showRun("pane-6", "7");
        leaveRun("pane-6");
        expect(viewOf("pane-6")).toMatchObject({ run: null, item: null, runFrom: null });
    });
});

describe("the pull request and issue lists", () => {
    it("start on every pull request and on open issues, and keep their filters apart", () => {
        expect(viewOf("pane-lists")).toMatchObject({ pullState: "all", issueState: "open" });
        setListState("pane-lists", "pulls", "closed");
        expect(viewOf("pane-lists")).toMatchObject({ pullState: "closed", issueState: "open" });
        setListState("pane-lists", "issues", "all");
        expect(viewOf("pane-lists")).toMatchObject({ pullState: "closed", issueState: "all" });
    });
});

describe("sections", () => {
    it("knows its own section names and nothing else", () => {
        expect(isSection("actions")).toBe(true);
        expect(isSection("wiki")).toBe(false);
        expect(isSection(3)).toBe(false);
    });

    it("needs a repository for every section but the inbox", () => {
        expect(needsRepo("inbox")).toBe(false);
        expect(needsRepo("releases")).toBe(true);
    });
});

describe("sameRepo", () => {
    const ref = { provider: HOST, owner: "nodelike", name: "sikemux" };

    it("matches the same repository on the same host, whichever account reads it", () => {
        expect(sameRepo(ref, { ...ref, account: "work" })).toBe(true);
        expect(sameRepo(ref, { ...ref, provider: "other.host" })).toBe(false);
        expect(sameRepo(ref, { ...ref, name: "tool" })).toBe(false);
    });

    it("never matches a missing repository", () => {
        expect(sameRepo(null, ref)).toBe(false);
        expect(sameRepo(ref, null)).toBe(false);
    });
});

describe("closing a pane", () => {
    it("forgets what the pane had open, and leaves other panes alone", () => {
        showRun("pane-closing", "7");
        showRun("pane-staying", "8");
        emit({ type: "pane-closed", paneId: "pane-closing" });
        emit({ type: "pane-closed", paneId: "pane-never-seen" });
        expect(viewOf("pane-closing").run).toBeNull();
        expect(viewOf("pane-staying").run).toBe("8");
    });
});
