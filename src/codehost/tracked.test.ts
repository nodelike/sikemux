import { describe, expect, it } from "vitest";
import { filterTracked, trackedContext, trackedItems, type TrackedItem } from "./tracked";
import type { Comment, Issue, Pull } from "./types";

const issue = (number: number, title: string, createdAt: string, extra: Partial<Issue> = {}): Issue => ({
    number,
    title,
    body: "",
    state: "open",
    stateReason: null,
    author: "ana",
    avatarUrl: null,
    createdAt,
    updatedAt: createdAt,
    closedAt: null,
    comments: 0,
    labels: [],
    assignees: [],
    url: `https://github.com/o/r/issues/${number}`,
    ...extra,
});

const pull = (number: number, title: string, createdAt: string): Pull =>
    ({
        number,
        title,
        body: "Makes it faster.",
        state: "open",
        draft: false,
        author: "bo",
        head: "fast",
        base: "main",
        createdAt,
        labels: [],
        mergedAt: null,
        url: `https://github.com/o/r/pull/${number}`,
    }) as unknown as Pull;

const comment = (id: number, body: string): Comment => ({
    id,
    author: `c${id}`,
    avatarUrl: null,
    authorAssociation: null,
    body,
    createdAt: `2026-09-0${id}T10:00:00Z`,
    url: null,
});

describe("trackedItems", () => {
    it("puts issues and pull requests together, newest first", () => {
        const items = trackedItems([issue(1, "Old", "2026-01-01"), issue(3, "New", "2026-03-01")], [pull(2, "Middle", "2026-02-01")]);
        expect(items.map((item) => `${item.kind}${item.number}`)).toEqual(["issue3", "pull2", "issue1"]);
    });

    it("keeps a pull request listed among the issues once, as a pull request", () => {
        const items = trackedItems([issue(2, "Middle", "2026-02-01")], [pull(2, "Middle", "2026-02-01")]);
        expect(items).toHaveLength(1);
        expect(items[0].kind).toBe("pull");
    });
});

describe("filterTracked", () => {
    const items: TrackedItem[] = trackedItems(
        [issue(12, "Login crashes on start", "2026-01-03"), issue(120, "Dark theme", "2026-01-02"), issue(7, "Crash in login form", "2026-01-01")],
        [],
    );

    it("matches digits against the start of a number", () => {
        expect(filterTracked(items, "12", 10).map((item) => item.number)).toEqual([12, 120]);
        expect(filterTracked(items, "2", 10)).toEqual([]);
    });

    it("matches every word against the title, in any order", () => {
        expect(filterTracked(items, "login crash", 10).map((item) => item.number)).toEqual([12, 7]);
        expect(filterTracked(items, "theme", 10).map((item) => item.number)).toEqual([120]);
    });

    it("offers the newest when nothing is typed yet, up to the limit", () => {
        expect(filterTracked(items, "", 2).map((item) => item.number)).toEqual([12, 120]);
    });
});

describe("trackedContext", () => {
    it("writes an issue's facts, body and latest comments", () => {
        const context = trackedContext(
            "issue",
            issue(12, "Login crashes", "2026-01-01", { body: "Steps to reproduce.", labels: [{ name: "bug", color: "f00" }] }),
            [1, 2, 3, 4, 5, 6].map((id) => comment(id, `note ${id}`)),
        );
        expect(context.uri).toBe("https://github.com/o/r/issues/12");
        expect(context.title).toBe("#12 Login crashes");
        expect(context.text).toContain("Issue #12: Login crashes\nState: open · Labels: bug · Author: @ana\nURL: https://github.com/o/r/issues/12");
        expect(context.text).toContain("Steps to reproduce.");
        expect(context.text).toContain("## Latest 5 of 6 comments");
        expect(context.text).not.toContain("note 1\n");
        expect(context.text).toContain("@c6, 2026-09-06:\nnote 6");
    });

    it("names a pull request's branches", () => {
        expect(trackedContext("pull", pull(9, "Faster boot", "2026-01-01"), []).text).toContain(
            "Pull request #9: Faster boot\nState: open · Author: @bo · Branch: fast → main",
        );
    });

    it("cuts a very long body short", () => {
        const context = trackedContext("issue", issue(1, "Long", "2026-01-01", { body: "x".repeat(20_000) }), []);
        expect(context.text.length).toBeLessThan(13_000);
        expect(context.text).toContain("8000 more characters");
    });
});
