import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { createRef } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Agent } from "../state/types";
import { ChatComposer } from "./ChatComposer";
import { PathRootsProvider } from "./FileRef";
import type { TrackedList } from "../codehost/tracked";
import { deliverToAgent } from "../agents/agentInbox";
import type { WorktreeSwitchState } from "./worktreeSwitch";

const repo = { provider: "github", owner: "o", name: "r", account: null };

const mocks = vi.hoisted(() => ({
    list: vi.fn(async () => ["src/main.ts", "src/lib/util.ts", "README.md"]),
    onSend: vi.fn((): boolean => true),
    tracked: vi.fn((): TrackedList => ({ state: "loading" })),
    load: vi.fn(async (_repo: unknown, kind: string, number: number) => ({
        uri: `https://github.com/o/r/${kind === "issue" ? "issues" : "pull"}/${number}`,
        title: `#${number} Login crashes`,
        text: `Issue #${number}: Login crashes`,
    })),
}));

vi.mock("../api/files", () => ({ filesApi: { list: mocks.list } }));
vi.mock("../codehost/tracked", async (importOriginal) => ({
    ...(await importOriginal<typeof import("../codehost/tracked")>()),
    useTrackedItems: mocks.tracked,
    loadTrackedContext: mocks.load,
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));

const agent: Agent = {
    id: "composer-agent",
    type: "codex",
    title: "Agent",
    startup: "codex",
    permissionMode: "workspace-write",
    launchState: "live",
};

function renderComposer(worktree?: { state: WorktreeSwitchState; toggle: () => void }) {
    render(
        <PathRootsProvider cwd="/repo">
            <ChatComposer
                agent={agent}
                paneRef={createRef()}
                visible
                connection="ready"
                running={false}
                steerable={false}
                commands={[]}
                setup={{}}
                awaitingPermission={false}
                agentLocked={false}
                changingConfig={false}
                changingPermissions={false}
                permissionApplied
                placeholder=""
                error={null}
                onError={() => {}}
                onSend={mocks.onSend}
                onSteerQueued={() => {}}
                onStop={() => {}}
                queuedCount={0}
                usage={null}
                onConfig={() => {}}
                history={[]}
                worktree={worktree}
            />
        </PathRootsProvider>,
    );
    return screen.getByRole("textbox", { name: "Message agent" });
}

function type(editor: HTMLElement, value: string) {
    fireEvent.change(editor, { target: { value, selectionStart: value.length } });
}

afterEach(() => {
    cleanup();
    mocks.onSend.mockClear();
});

describe("ChatComposer @ picker", () => {
    it("lists project files and folders matching what follows the @", async () => {
        const editor = renderComposer();
        type(editor, "look at @util");
        expect(await screen.findByRole("option", { name: /util\.ts/ })).toBeInTheDocument();
        expect(screen.queryByRole("option", { name: /README/ })).not.toBeInTheDocument();
        type(editor, "look at @lib");
        expect(await screen.findByRole("option", { name: /^lib/ })).toBeInTheDocument();
    });

    it("takes the token out of the text and attaches the chosen path", async () => {
        const editor = renderComposer();
        type(editor, "look at @main please");
        fireEvent.change(editor, { target: { value: "look at @main please", selectionStart: 13 } });
        await screen.findByRole("option", { name: /main\.ts/ });
        fireEvent.keyDown(editor, { key: "Enter" });
        expect(editor).toHaveValue("look at please");
        expect(screen.getByTitle("/repo/src/main.ts")).toBeInTheDocument();

        fireEvent.keyDown(editor, { key: "Enter" });
        expect(mocks.onSend).toHaveBeenCalledWith({ text: "look at please", paths: ["/repo/src/main.ts"], context: [] }, false);
    });

    it("leaves the @ in place when Escape dismisses the picker", async () => {
        const editor = renderComposer();
        type(editor, "@READ");
        await screen.findByRole("option", { name: /README/ });
        fireEvent.keyDown(editor, { key: "Escape" });
        await act(async () => {});
        expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
        expect(editor).toHaveValue("@READ");
    });

    it("keeps an email address as text", async () => {
        const editor = renderComposer();
        type(editor, "mail me@main");
        await act(async () => {});
        expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    });
});

describe("ChatComposer # picker", () => {
    const items = [
        { kind: "issue" as const, number: 12, title: "Login crashes", createdAt: "2026-02-01", url: "https://github.com/o/r/issues/12" },
        { kind: "pull" as const, number: 9, title: "Faster boot", createdAt: "2026-01-01", url: "https://github.com/o/r/pull/9" },
    ];

    it("lists open issues and pull requests, filtered by number or words", async () => {
        mocks.tracked.mockReturnValue({ state: "ready", repo, items });
        const editor = renderComposer();
        type(editor, "#");
        expect(await screen.findByRole("option", { name: /#12.*Login crashes/ })).toBeInTheDocument();
        expect(screen.getByRole("option", { name: /#9.*Faster boot/ })).toBeInTheDocument();
        type(editor, "#boot");
        expect(screen.queryByRole("option", { name: /#12/ })).not.toBeInTheDocument();
        type(editor, "#1");
        expect(screen.getByRole("option", { name: /#12/ })).toBeInTheDocument();
        expect(screen.queryByRole("option", { name: /#9/ })).not.toBeInTheDocument();
    });

    it("says why there is nothing to pick", async () => {
        mocks.tracked.mockReturnValue({ state: "unavailable", message: "Sign in to GitHub in the Git pane to pick from its issues" });
        const editor = renderComposer();
        type(editor, "fix #");
        expect(await screen.findByText("Sign in to GitHub in the Git pane to pick from its issues")).toBeInTheDocument();
    });

    it("adds a chip, and reads the issue in full when the message is sent", async () => {
        mocks.tracked.mockReturnValue({ state: "ready", repo, items });
        const editor = renderComposer();
        type(editor, "fix #12");
        fireEvent.keyDown(await screen.findByRole("textbox", { name: "Message agent" }), { key: "Enter" });
        expect(editor).toHaveValue("fix ");
        expect(screen.getByTitle("https://github.com/o/r/issues/12")).toHaveTextContent("#12 Login crashes");

        fireEvent.keyDown(editor, { key: "Enter" });
        await waitFor(() =>
            expect(mocks.onSend).toHaveBeenCalledWith(
                {
                    text: "fix",
                    paths: [],
                    context: [{ uri: "https://github.com/o/r/issues/12", title: "#12 Login crashes", text: "Issue #12: Login crashes" }],
                },
                false,
            ),
        );
        expect(mocks.load).toHaveBeenCalledWith(repo, "issue", 12);
        await waitFor(() => expect(screen.queryByTitle("https://github.com/o/r/issues/12")).not.toBeInTheDocument());
    });

    it("lets a chip be removed", async () => {
        mocks.tracked.mockReturnValue({ state: "ready", repo, items });
        const editor = renderComposer();
        type(editor, "#9");
        fireEvent.keyDown(editor, { key: "Enter" });
        fireEvent.click(screen.getByRole("button", { name: "Remove #9" }));
        expect(screen.queryByTitle("https://github.com/o/r/pull/9")).not.toBeInTheDocument();
    });
});

describe("ChatComposer deliveries", () => {
    it("shows a delivered issue as a chip and sends it as it came", async () => {
        const editor = renderComposer();
        const issue = { uri: "https://github.com/o/r/issues/4", title: "#4 Flaky test", text: "Issue #4: Flaky test" };
        act(() => deliverToAgent("composer-agent", { context: [issue] }));
        expect(screen.getByTitle(issue.uri)).toHaveTextContent("#4 Flaky test");
        fireEvent.keyDown(editor, { key: "Enter" });
        expect(mocks.onSend).toHaveBeenCalledWith({ text: "", paths: [], context: [issue] }, false);
        expect(mocks.load).not.toHaveBeenCalled();
    });
});

describe("ChatComposer Worktree switch", () => {
    const toggle = vi.fn();

    it("is absent outside a git repository", () => {
        renderComposer({ state: { kind: "hidden" }, toggle });
        expect(screen.queryByRole("button", { name: "worktree" })).not.toBeInTheDocument();
    });

    it("leaves the choice to the project strip above it", () => {
        renderComposer({ state: { kind: "choosing", on: false }, toggle });
        expect(screen.queryByRole("button", { name: "worktree" })).not.toBeInTheDocument();
    });

    it("names the branch of an agent already in a worktree", async () => {
        renderComposer({ state: { kind: "in", branch: "sikemux/fix-pty", path: "/code/app.worktrees/fix-pty" }, toggle });
        const button = await screen.findByRole("button", { name: "sikemux/fix-pty" });
        expect(button).toBeDisabled();
        expect(button).toHaveAttribute("aria-pressed", "true");
        expect(button.title).toContain("/code/app.worktrees/fix-pty");
    });

    it("holds the message back while the worktree is being set up", async () => {
        const editor = renderComposer({ state: { kind: "preparing", step: "Running Install" }, toggle });
        type(editor, "hello");
        fireEvent.keyDown(editor, { key: "Enter" });
        expect(mocks.onSend).not.toHaveBeenCalled();
        expect((await screen.findByRole("button", { name: "worktree" })).title).toBe("Running Install…");
    });
});
