import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SlackStatus } from "../api";

const api = vi.hoisted(() => ({
    status: vi.fn(),
    signIn: vi.fn(),
    signOut: vi.fn(),
    setDefault: vi.fn(),
    channels: vi.fn(),
    history: vi.fn(),
    thread: vi.fn(),
    threadOf: vi.fn(),
    reply: vi.fn(),
}));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), slackApi: api }));

import { invalidate } from "../../../plugin-api/resources";
import { SlackPane } from "./SlackPane";

const signedIn: SlackStatus = {
    configured: true,
    ok: true,
    authFailed: false,
    message: null,
    workspaces: [{ id: "T1", name: "Acme", domain: "acme.slack.com", user: "ankit", userId: "U2", isDefault: true }],
};

const message = (ts: string, name: string, text: string, replyCount = 0) => ({
    ts,
    user: "U1",
    name,
    bot: false,
    text,
    at: "2026-10-10T09:00:00Z",
    threadTs: replyCount ? ts : null,
    replyCount,
    files: [],
});

let pane = 0;
const renderPane = () => render(<SlackPane paneId={`slack-${++pane}`} active />);

afterEach(() => {
    cleanup();
    invalidate((kind) => kind.startsWith("slack."));
});

beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    api.status.mockResolvedValue(signedIn);
    api.channels.mockResolvedValue([
        { id: "C9", name: "deploys", kind: "channel", updated: null },
        { id: "D1", name: "Irwan", kind: "dm", updated: null },
    ]);
    api.history.mockResolvedValue([message("100.000001", "Irwan", "Deploy failed on staging", 2), message("101.000001", "Vikas", "ok")]);
    api.thread.mockResolvedValue({
        workspace: "T1",
        channel: "C9",
        ts: "100.000001",
        permalink: "https://acme.slack.com/archives/C9/p100000001",
        truncated: false,
        messages: [message("100.000001", "Irwan", "Deploy failed on staging", 2), message("100.000002", "Ankit", "Looking now")],
    });
    api.reply.mockResolvedValue({ channel: "C9", ts: "100.000003", permalink: null });
});

describe("SlackPane", () => {
    it("asks for a token when no workspace is signed in", async () => {
        api.status.mockResolvedValue({ ...signedIn, configured: false, ok: false, workspaces: [] });
        renderPane();
        expect(await screen.findByText("Connect Slack")).toBeInTheDocument();
    });

    it("lists channels and direct messages, and shows a channel's messages", async () => {
        renderPane();
        const sidebar = await screen.findByRole("navigation", { name: "Slack channels" });
        expect(within(sidebar).getByText("Channels")).toBeInTheDocument();
        expect(within(sidebar).getByText("Direct messages")).toBeInTheDocument();
        fireEvent.click(await within(sidebar).findByText("deploys"));
        expect(await screen.findByText("Deploy failed on staging")).toBeInTheDocument();
        expect(api.history).toHaveBeenCalledWith("T1", "C9");
        expect(screen.getByRole("button", { name: "2 replies" })).toBeInTheDocument();
    });

    it("opens a thread beside the channel and replies in it", async () => {
        renderPane();
        fireEvent.click(await screen.findByText("deploys"));
        fireEvent.click(await screen.findByRole("button", { name: "2 replies" }));
        const thread = await screen.findByRole("complementary", { name: "Thread" });
        expect(await within(thread).findByText("Looking now")).toBeInTheDocument();
        expect(api.thread).toHaveBeenCalledWith("T1", "C9", "100.000001");
        fireEvent.change(within(thread).getByRole("textbox", { name: "Reply in the thread" }), { target: { value: " On it " } });
        await act(async () => fireEvent.click(within(thread).getByRole("button", { name: "Reply" })));
        expect(api.reply).toHaveBeenCalledWith("T1", "C9", "100.000001", "On it");
    });

    it("opens the thread a pasted link points at", async () => {
        api.threadOf.mockResolvedValue({ workspace: "T1", channel: "C9", ts: "100.000001", permalink: null, truncated: false, messages: [] });
        renderPane();
        const box = await screen.findByRole("textbox", { name: "Open a Slack message link" });
        fireEvent.change(box, { target: { value: "https://acme.slack.com/archives/C9/p100000001" } });
        await act(async () => fireEvent.keyDown(box, { key: "Enter" }));
        expect(api.threadOf).toHaveBeenCalledWith("https://acme.slack.com/archives/C9/p100000001");
        expect(await screen.findByRole("complementary", { name: "Thread" })).toBeInTheDocument();
    });
});
