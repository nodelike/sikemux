import { useState } from "react";
import { act, cleanup, fireEvent, render, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Notification } from "../api";

const api = vi.hoisted(() => ({
    inbox: vi.fn(),
    status: vi.fn(() =>
        Promise.resolve({ id: "work", ok: true, login: "me", avatarUrl: null, host: "example.test", canWriteCi: true, warning: null }),
    ),
    markRead: vi.fn(),
    issue: vi.fn(),
    timeline: vi.fn(() => Promise.resolve([])),
}));

import { invalidate } from "../../plugin-api/resources";
import { AccountProvider } from "../registry";
import { InHost, registerTestHost, TEST_HOST } from "../testHost";
import { InboxDetail, InboxList } from "./InboxView";

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

const list = () => document.querySelector(".inbox-list") as HTMLElement;
const right = () => document.querySelector(".inbox-detail") as HTMLElement;

/** The inbox as the git pane lays it out: the list in its side panel, the open notification beside it. */
function Inbox() {
    const [shown, setShown] = useState<Notification | null>(null);
    return (
        <>
            <div className="inbox-list">
                <InboxList shownId={shown?.id ?? null} onOpen={setShown} active />
            </div>
            <div className="inbox-detail">{shown && <InboxDetail item={shown} active />}</div>
        </>
    );
}

async function renderInbox(items: Notification[] = [item()]) {
    api.inbox.mockResolvedValue(items);
    const view = render(
        <InHost host={host}>
            <AccountProvider value="work">
                <Inbox />
            </AccountProvider>
        </InHost>,
    );
    await act(async () => {});
    return view;
}

beforeEach(() => {
    invalidate(() => true);
    api.inbox.mockReset();
    api.markRead.mockReset().mockResolvedValue(undefined);
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
});

afterEach(cleanup);

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
});
