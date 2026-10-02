import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Notification, RepoRef } from "../api";

const api = vi.hoisted(() => ({
    inbox: vi.fn(),
    markRead: vi.fn(),
    markAllRead: vi.fn(),
    issue: vi.fn(),
    timeline: vi.fn(() => Promise.resolve([])),
}));
const shell = vi.hoisted(() => ({ openUrl: vi.fn(() => Promise.resolve()) }));

vi.mock("../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), openUrl: shell.openUrl }));
vi.mock("./PullsView", () => ({
    PullRight: ({ repo, number, login }: { repo: RepoRef; number: number; login: string | null }) => (
        <div data-testid="pull">{`${repo.provider} ${repo.owner}/${repo.name}#${number} as ${login}`}</div>
    ),
}));

import { invalidate } from "../../plugin-api/resources";
import { useToasts } from "../../state/toast";
import { AccountProvider } from "../registry";
import { InHost, registerTestHost, TEST_HOST } from "../testHost";
import { InboxView, reasonLabel } from "./InboxView";

const host = registerTestHost(api);

const item = (overrides: Partial<Notification> = {}): Notification => ({
    id: "n1",
    title: "The log jumps",
    kind: "Issue",
    reason: "mention",
    repo: "nodelike/sikemux",
    number: 5,
    unread: true,
    updatedAt: "2026-01-01T12:00:00Z",
    url: "https://github.com/nodelike/sikemux/issues/5",
    ...overrides,
});

const toasts = () => useToasts.getState().toasts.map((toast) => toast.text);
const list = () => document.querySelector(".git-left") as HTMLElement;
const right = () => document.querySelector(".git-right") as HTMLElement;

async function renderInbox(items: Notification[] = [item()]) {
    api.inbox.mockResolvedValue(items);
    const view = render(
        <InHost host={host}>
            <AccountProvider value="work">
                <InboxView paneId="pane" login="me" active />
            </AccountProvider>
        </InHost>,
    );
    await act(async () => {});
    return view;
}

beforeEach(() => {
    invalidate(() => true);
    useToasts.setState({ toasts: [] });
    api.inbox.mockReset();
    api.markRead.mockReset().mockResolvedValue(undefined);
    api.markAllRead.mockReset().mockResolvedValue(undefined);
    api.issue.mockReset().mockResolvedValue({
        number: 5,
        title: "The log jumps",
        body: "",
        state: "open",
        stateReason: null,
        author: null,
        avatarUrl: null,
        createdAt: "2026-01-01T12:00:00Z",
        updatedAt: "2026-01-01T12:00:00Z",
        closedAt: null,
        comments: 0,
        labels: [],
        assignees: [],
        url: "",
    });
    shell.openUrl.mockClear();
});

afterEach(cleanup);

describe("reasonLabel", () => {
    it("says why a notification came in words, and makes do with a reason it does not know", () => {
        expect(reasonLabel("review_requested")).toBe("Review requested");
        expect(reasonLabel("manual")).toBe("Subscribed");
        expect(reasonLabel("security_alert")).toBe("security alert");
    });
});

describe("the inbox list", () => {
    it("reads the unread notifications of the account in use", async () => {
        await renderInbox([item(), item({ id: "n2", title: "Release out", kind: "Release", number: null, unread: false, reason: "subscribed" })]);
        expect(api.inbox).toHaveBeenCalledWith("work", false);
        expect(within(list()).getByText("1 unread")).toBeTruthy();
        const [first, second] = within(list()).getAllByRole("button", { name: /The log jumps|Release out/ });
        expect(first.dataset.unread).toBe("1");
        expect(within(first).getByText("#5")).toBeTruthy();
        expect(within(first).getByText("Mentioned")).toBeTruthy();
        expect(second.dataset.unread).toBe("0");
        expect(second.querySelector(".gha-item-number")).toBeNull();
    });

    it("includes read notifications on request", async () => {
        await renderInbox();
        fireEvent.click(screen.getByRole("checkbox", { name: "Include read" }));
        await act(async () => {});
        expect(api.inbox).toHaveBeenLastCalledWith("work", true);
    });

    it("says when nothing is waiting, and offers nothing to clear", async () => {
        await renderInbox([]);
        expect(within(list()).getByText("Nothing waiting")).toBeTruthy();
        expect(screen.queryByRole("button", { name: "Mark all read" })).toBeNull();
        expect(within(right()).getByText("Pick a notification to read it.")).toBeTruthy();
    });

    it("shows placeholders while it loads", async () => {
        let answer: (items: Notification[]) => void = () => {};
        api.inbox.mockReturnValue(new Promise((resolve) => (answer = resolve)));
        render(
            <InHost host={host}>
                <InboxView paneId="pane" login="me" active />
            </InHost>,
        );
        await act(async () => {});
        expect(screen.getByLabelText("Loading notifications")).toBeTruthy();
        await act(async () => answer([]));
        expect(screen.queryByLabelText("Loading notifications")).toBeNull();
    });

    it("says why the notifications could not be read, and reads them again on request", async () => {
        api.inbox.mockRejectedValueOnce("Bad credentials").mockResolvedValue([item()]);
        render(
            <InHost host={host}>
                <InboxView paneId="pane" login="me" active />
            </InHost>,
        );
        await act(async () => {});
        expect(screen.getByText("Could not read notifications")).toBeTruthy();
        expect(screen.getByText("Bad credentials")).toBeTruthy();
        fireEvent.click(screen.getByRole("button", { name: "Try again" }));
        await act(async () => {});
        expect(within(list()).getByText("1 unread")).toBeTruthy();
    });

    it("clears the inbox", async () => {
        await renderInbox();
        api.inbox.mockClear();
        fireEvent.click(screen.getByRole("button", { name: "Mark all read" }));
        await act(async () => {});
        expect(api.markAllRead).toHaveBeenCalledWith("work");
        expect(toasts()).toContain("Inbox cleared");
        expect(api.inbox).toHaveBeenCalled();
    });

    it("says why the inbox could not be cleared", async () => {
        api.markAllRead.mockRejectedValue(new Error("offline"));
        await renderInbox();
        fireEvent.click(screen.getByRole("button", { name: "Mark all read" }));
        await act(async () => {});
        expect(toasts()).toContain("Could not clear the inbox: offline");
    });
});

describe("reading a notification", () => {
    it("marks an unread one read as it opens", async () => {
        await renderInbox();
        api.inbox.mockClear();
        fireEvent.click(within(list()).getByRole("button", { name: /The log jumps/ }));
        await act(async () => {});
        expect(api.markRead).toHaveBeenCalledWith("work", "n1");
        expect(api.inbox).toHaveBeenCalled();
        expect(within(list()).getByRole("button", { name: /The log jumps/ }).dataset.on).toBe("1");
    });

    it("does not mark one already read", async () => {
        await renderInbox([item({ unread: false })]);
        fireEvent.click(within(list()).getByRole("button", { name: /The log jumps/ }));
        await act(async () => {});
        expect(api.markRead).not.toHaveBeenCalled();
    });

    it("reads an issue in full, in its own repository", async () => {
        await renderInbox([item({ repo: "other/tool" })]);
        fireEvent.click(within(list()).getByRole("button", { name: /The log jumps/ }));
        await act(async () => {});
        expect(api.issue).toHaveBeenCalledWith({ provider: TEST_HOST, owner: "other", name: "tool" }, 5);
        expect(within(right()).getByRole("heading", { name: "The log jumps" })).toBeTruthy();
    });

    it("reads a pull request in full, as the person signed in", async () => {
        await renderInbox([item({ kind: "PullRequest", number: 31, title: "the run page" })]);
        fireEvent.click(within(list()).getByRole("button", { name: /the run page/ }));
        expect(screen.getByTestId("pull").textContent).toBe(`${TEST_HOST} nodelike/sikemux#31 as me`);
    });

    it("sends anything else to the host", async () => {
        await renderInbox([item({ kind: "Release", number: null, title: "v2 is out", reason: "subscribed", url: "https://github.com/r/1" })]);
        fireEvent.click(within(list()).getByRole("button", { name: /v2 is out/ }));
        expect(within(right()).getByText("nodelike/sikemux · Subscribed")).toBeTruthy();
        fireEvent.click(within(right()).getByRole("button", { name: "Open on Test host" }));
        expect(shell.openUrl).toHaveBeenCalledWith("https://github.com/r/1");
    });

    it("offers no way out to the host when it gave no link", async () => {
        await renderInbox([item({ kind: "Discussion", number: null, title: "Ideas", url: null })]);
        fireEvent.click(within(list()).getByRole("button", { name: /Ideas/ }));
        expect(within(right()).queryByRole("button")).toBeNull();
    });
});
