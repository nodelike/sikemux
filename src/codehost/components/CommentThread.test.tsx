import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TimelineItem } from "../api";

const api = vi.hoisted(() => ({
    timeline: vi.fn(),
    addComment: vi.fn(),
    reviewPull: vi.fn(),
    image: vi.fn(() => new Promise<string>(() => {})),
}));

import { invalidate } from "../../plugin-api/resources";
import { useToasts } from "../../state/toast";
import type { CodeHost } from "../registry";
import { InHost, registerTestHost, TEST_HOST } from "../testHost";
import { CommentThread, type Post } from "./CommentThread";

const host = registerTestHost(api);
const repo = { provider: TEST_HOST, owner: "nodelike", name: "sikemux" };
const NOW = Date.parse("2026-01-10T12:00:00Z");
const AN_HOUR_AGO = "2026-01-10T11:00:00Z";

const item = (kind: string, extra: Partial<TimelineItem> = {}): TimelineItem => ({
    kind,
    id: null,
    actor: "ada",
    avatarUrl: null,
    association: null,
    at: AN_HOUR_AGO,
    body: null,
    state: null,
    sha: null,
    message: null,
    subject: null,
    ...extra,
});

type ThreadProps = Partial<React.ComponentProps<typeof CommentThread>> & { on?: CodeHost };

function thread({ on = host, ...props }: ThreadProps = {}) {
    return render(
        <InHost host={on}>
            <CommentThread repo={repo} number={12} active now={NOW} {...props} />
        </InHost>,
    );
}

const spoken = (node: Element) => {
    const copy = node.cloneNode(true) as Element;
    copy.querySelectorAll("[aria-hidden]").forEach((hidden) => hidden.remove());
    return copy.textContent?.replace(/\s+/g, " ").trim();
};
const events = () => Array.from(document.querySelectorAll(".gha-tl-event .gha-tl-text")).map(spoken);
const toasts = () => useToasts.getState().toasts.map((toast) => `${toast.kind}: ${toast.text}`);

beforeEach(() => {
    invalidate(() => true);
    for (const mock of Object.values(api)) mock.mockReset();
    api.image.mockReturnValue(new Promise(() => {}));
    api.timeline.mockResolvedValue([]);
    api.addComment.mockResolvedValue(undefined);
    api.reviewPull.mockResolvedValue(undefined);
    useToasts.setState({ toasts: [] });
});

afterEach(cleanup);

describe("the timeline", () => {
    it("tells each kind of event in words", async () => {
        api.timeline.mockResolvedValue([
            item("review_requested", { id: 1, subject: "grace" }),
            item("review_request_removed", { id: 2, subject: "grace" }),
            item("head_ref_force_pushed", { id: 3, sha: "abcdef1234567" }),
            item("head_ref_force_pushed", { id: 4 }),
            item("closed", { id: 5, state: "not_planned" }),
            item("closed", { id: 6 }),
            item("reopened", { id: 7 }),
            item("labeled", { id: 8, subject: "bug" }),
            item("unlabeled", { id: 9, subject: "bug" }),
            item("assigned", { id: 10, subject: "grace" }),
            item("unassigned", { id: 11, subject: "grace" }),
            item("renamed", { id: 12, subject: "A better title" }),
            item("ready_for_review", { id: 13 }),
            item("convert_to_draft", { id: 14 }),
            item("head_ref_deleted", { id: 15 }),
            item("head_ref_restored", { id: 16 }),
            item("referenced", { id: 17, sha: "1234567890" }),
            item("referenced", { id: 18 }),
            item("cross-referenced", { id: 19, subject: "nodelike/other#4" }),
            item("subscribed", { id: 20 }),
            item("closed", { id: 21, actor: null }),
        ]);
        thread();
        await waitFor(() => expect(events().length).toBeGreaterThan(0));
        expect(events()).toEqual([
            "ada requested a review from grace 1h ago",
            "ada removed the review request for grace 1h ago",
            "ada force-pushed the branch to abcdef1 1h ago",
            "ada force-pushed the branch 1h ago",
            "ada closed this as not planned 1h ago",
            "ada closed this 1h ago",
            "ada reopened this 1h ago",
            "ada added the bug label 1h ago",
            "ada removed the bug label 1h ago",
            "ada assigned grace 1h ago",
            "ada unassigned grace 1h ago",
            "ada changed the title to A better title 1h ago",
            "ada marked this ready for review 1h ago",
            "ada marked this as a draft 1h ago",
            "ada deleted the branch 1h ago",
            "ada restored the branch 1h ago",
            "ada referenced this in commit 1234567 1h ago",
            "ada referenced this 1h ago",
            "ada mentioned this in nodelike/other#4 1h ago",
            "someone closed this 1h ago",
        ]);
    });

    it("names the branch a merge went into, and the base branch when it does not know it", async () => {
        api.timeline.mockResolvedValue([item("merged", { id: 1, sha: "feedface00" })]);
        thread({ base: "main" });
        await waitFor(() => expect(events()).toEqual(["ada merged commit feedfac into main 1h ago"]));
        cleanup();
        invalidate(() => true);
        api.timeline.mockResolvedValue([item("merged", { id: 1 })]);
        thread();
        await waitFor(() => expect(events()).toEqual(["ada merged into the base branch 1h ago"]));
    });

    it("shows comments with the writer's role, and says so when one is empty", async () => {
        api.timeline.mockResolvedValue([
            item("commented", { id: 1, body: "Looks good to me", association: "FIRST_TIME_CONTRIBUTOR" }),
            item("commented", { id: 2, body: "   ", association: "NONE", actor: "grace" }),
        ]);
        thread();
        expect(await screen.findByText("Looks good to me")).toBeTruthy();
        expect(screen.getByText("First-time contributor")).toBeTruthy();
        expect(screen.getByText("No description provided.")).toBeTruthy();
        expect(document.querySelectorAll(".gha-role")).toHaveLength(1);
        expect(screen.getAllByText("commented")).toHaveLength(2);
    });

    it("shows a review with words as a post, and one without as an event", async () => {
        api.timeline.mockResolvedValue([
            item("reviewed", { id: 1, state: "changes_requested", body: "Please rename this" }),
            item("reviewed", { id: 2, state: "approved" }),
            item("reviewed", { id: 3, state: "changes_requested" }),
            item("reviewed", { id: 4, state: null }),
            item("reviewed", { id: 5, state: "pending" }),
            item("reviewed", { id: 6, state: "custom_state", body: "odd" }),
        ]);
        thread();
        expect(await screen.findByText("Please rename this")).toBeTruthy();
        const verdicts = Array.from(document.querySelectorAll(".gha-review-state")).map((node) => [
            node.getAttribute("data-state"),
            node.textContent,
        ]);
        expect(verdicts).toEqual([
            ["CHANGES_REQUESTED", "requested changes"],
            ["APPROVED", "approved these changes"],
            ["CHANGES_REQUESTED", "requested changes"],
            ["COMMENTED", "reviewed"],
            ["PENDING", "reviewed"],
            ["CUSTOM_STATE", "custom_state"],
        ]);
        const tones = Array.from(document.querySelectorAll(".gha-tl-badge")).map((node) => node.getAttribute("data-tone"));
        expect(tones).toEqual(["live", "danger", null, null]);
    });

    it("groups commits pushed together, and names only the first author", async () => {
        api.timeline.mockResolvedValue([
            item("committed", { sha: "aaaaaaa1111", message: "feat: one\n\nwith a body" }),
            item("committed", { sha: "bbbbbbb2222", message: "fix: two", actor: "grace" }),
            item("commented", { id: 1, body: "between" }),
            item("committed", { sha: "ccccccc3333", message: null }),
        ]);
        thread();
        await screen.findByText("between");
        const pushes = Array.from(document.querySelectorAll(".gha-tl-commits .gha-tl-text")).map((node) =>
            Array.from(node.children).map(spoken).join(" "),
        );
        expect(pushes).toEqual(["ada and others added 2 commits 1h ago", "ada added 1 commit 1h ago"]);
        expect(Array.from(document.querySelectorAll(".gha-tl-commit-message")).map((node) => node.textContent)).toEqual([
            "feat: one",
            "fix: two",
            "",
        ]);
    });

    it("copies a commit's full sha", async () => {
        const writeText = vi.fn(() => Promise.resolve());
        Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
        api.timeline.mockResolvedValue([item("committed", { sha: "aaaaaaa1111", message: "feat: one" })]);
        thread();
        await userEvent.click(await screen.findByRole("button", { name: "aaaaaaa" }));
        expect(writeText).toHaveBeenCalledWith("aaaaaaa1111");
        await waitFor(() => expect(toasts()).toContain("success: Copied aaaaaaa"));
    });

    it("leaves commits out of a pull request that lists them beside the thread", async () => {
        api.timeline.mockResolvedValue([item("committed", { sha: "aaaaaaa1111", message: "feat: one" }), item("reopened", { id: 1 })]);
        thread({ withoutCommits: true });
        await waitFor(() => expect(events()).toEqual(["ada reopened this 1h ago"]));
        expect(document.querySelector(".gha-tl-commits")).toBeNull();
    });

    it("opens with the description, and draws whatever sits under the timeline", async () => {
        const opening: Post = { key: "o", author: null, avatarUrl: null, association: "OWNER", at: null, body: "The description", review: null };
        thread({ opening, children: <div>merge box</div> });
        expect(await screen.findByText("The description")).toBeTruthy();
        expect(screen.getByText("someone")).toBeTruthy();
        expect(screen.getByText("Owner")).toBeTruthy();
        expect(screen.getByText("merge box")).toBeTruthy();
    });

    it("reads nothing while the pane is hidden", () => {
        thread({ active: false });
        expect(api.timeline).not.toHaveBeenCalled();
    });
});

describe("the composer", () => {
    it("sends a comment only once something is written, trimmed, then clears", async () => {
        thread();
        const box = screen.getByPlaceholderText("Leave a comment");
        const send = screen.getByRole("button", { name: "Comment" });
        expect((send as HTMLButtonElement).disabled).toBe(true);
        await userEvent.type(box, "  hello  ");
        await userEvent.click(send);
        expect(api.addComment).toHaveBeenCalledWith(repo, 12, "hello");
        await waitFor(() => expect((box as HTMLTextAreaElement).value).toBe(""));
        expect(toasts()).toContain("success: Comment added");
        await waitFor(() => expect(api.timeline).toHaveBeenCalledTimes(2));
    });

    it("keeps the comment and says why when it cannot be sent", async () => {
        let fail: (error: unknown) => void = () => {};
        api.addComment.mockReturnValue(new Promise((_resolve, reject) => (fail = reject)));
        thread();
        const box = screen.getByPlaceholderText("Leave a comment");
        await userEvent.type(box, "hello");
        await userEvent.click(screen.getByRole("button", { name: "Comment" }));
        expect(screen.getByRole("button", { name: "Sending…" })).toBeTruthy();
        fail(new Error("offline"));
        await waitFor(() => expect(toasts()).toContain("error: Could not add the comment: offline"));
        expect((box as HTMLTextAreaElement).value).toBe("hello");
        expect(screen.getByRole("button", { name: "Comment" })).toBeTruthy();
    });

    it("offers approving and asking for changes on someone else's pull request", async () => {
        thread({ review: { mine: false } });
        const box = screen.getByPlaceholderText("Leave a comment or a review");
        expect((screen.getByRole("button", { name: "Request changes" }) as HTMLButtonElement).disabled).toBe(true);
        expect((screen.getByRole("button", { name: "Approve" }) as HTMLButtonElement).disabled).toBe(false);
        await userEvent.type(box, "rename it");
        await userEvent.click(screen.getByRole("button", { name: "Request changes" }));
        expect(api.reviewPull).toHaveBeenCalledWith(repo, 12, "REQUEST_CHANGES", "rename it");
        await waitFor(() => expect(toasts()).toContain("success: Asked for changes on #12"));
        expect((box as HTMLTextAreaElement).value).toBe("");
    });

    it("approves without any words, saying so while it does", async () => {
        let finish: () => void = () => {};
        api.reviewPull.mockReturnValue(new Promise<void>((resolve) => (finish = resolve)));
        thread({ review: { mine: false } });
        await userEvent.click(screen.getByRole("button", { name: "Approve" }));
        expect(api.reviewPull).toHaveBeenCalledWith(repo, 12, "APPROVE", "");
        expect((screen.getByRole("button", { name: "Approving…" }) as HTMLButtonElement).disabled).toBe(true);
        finish();
        await waitFor(() => expect(toasts()).toContain("success: Approved #12"));
    });

    it("reports a review that could not be sent", async () => {
        api.reviewPull.mockRejectedValue({ category: "http", message: "http 422: cannot approve" });
        thread({ review: { mine: false } });
        await userEvent.click(screen.getByRole("button", { name: "Approve" }));
        await waitFor(() => expect(toasts()).toContain("error: Could not send the review: http 422: cannot approve"));
    });

    it("explains why there is no approving your own pull request", () => {
        thread({ review: { mine: true }, extraActions: <button type="button">Close pull request</button> });
        expect(screen.getByText("Test host does not let you approve your own pull request.")).toBeTruthy();
        expect(screen.queryByRole("button", { name: "Approve" })).toBeNull();
        expect(screen.getByRole("button", { name: "Close pull request" })).toBeTruthy();
    });

    it("leaves out asking for changes on a host that has no such thing", () => {
        const plain = { ...host, capabilities: { ...host.capabilities, pulls: { ...host.capabilities.pulls, requestChanges: false } } };
        thread({ on: plain, review: { mine: false } });
        expect(screen.queryByRole("button", { name: "Request changes" })).toBeNull();
        expect(screen.getByRole("button", { name: "Approve" })).toBeTruthy();
    });
});
