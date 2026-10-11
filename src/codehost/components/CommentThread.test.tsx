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
import { CommentThread } from "./CommentThread";

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
        expect(api.addComment).toHaveBeenCalledWith(repo, 12, "hello", "pull");
        await waitFor(() => expect((box as HTMLTextAreaElement).value).toBe(""));
        expect(toasts()).toContain("success: Comment added");
        await waitFor(() => expect(api.timeline).toHaveBeenCalledTimes(2));
    });

    it("tells the host an issue's thread is an issue's, for hosts that number them apart", async () => {
        api.addComment.mockResolvedValue(undefined);
        thread({ of: "issue" });
        await waitFor(() => expect(api.timeline).toHaveBeenCalledWith(repo, 12, "issue"));
        await userEvent.type(screen.getByPlaceholderText("Leave a comment"), "seen");
        await userEvent.click(screen.getByRole("button", { name: "Comment" }));
        expect(api.addComment).toHaveBeenCalledWith(repo, 12, "seen", "issue");
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
