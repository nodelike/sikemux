import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { createRef, type RefObject } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Agent } from "../state/types";
import { ChatComposer } from "./ChatComposer";
import { PathRootsProvider } from "./FileRef";
import type { TrackedList } from "../codehost/tracked";
import { deliverToAgent } from "../agents/agentInbox";
import type { WorktreeSwitchState } from "./worktreeSwitch";
import { handleVoiceEvent, toggleDictation, useVoice } from "../voice/dictation";
import { setState } from "../state/store";

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
const voice = vi.hoisted(() => ({
    status: vi.fn(async () => ({ supported: true, installed: true, reason: null })),
    prepare: vi.fn(async () => {}),
    start: vi.fn(async () => {}),
    stop: vi.fn(async () => {}),
    cancel: vi.fn(async () => {}),
    shutdown: vi.fn(async () => {}),
    subscribe: vi.fn(async () => () => {}),
}));
vi.mock("../api/voice", () => ({ voiceApi: voice }));

const agent: Agent = {
    id: "composer-agent",
    type: "codex",
    title: "Agent",
    startup: "codex",
    permissionMode: "workspace-write",
    launchState: "live",
};

function renderComposer(worktree?: { state: WorktreeSwitchState; toggle: () => void }, paneRef: RefObject<HTMLDivElement | null> = createRef()) {
    render(
        <PathRootsProvider cwd="/repo">
            <ChatComposer
                agent={agent}
                paneRef={paneRef}
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

    it("leaves the choice to the project strip above it", () => {
        renderComposer({ state: { kind: "choosing", on: false }, toggle });
        expect(screen.queryByRole("button", { name: "worktree" })).not.toBeInTheDocument();
    });

    it("holds the message back while the worktree is being set up", async () => {
        const editor = renderComposer({ state: { kind: "preparing", step: "Running Install" }, toggle });
        type(editor, "hello");
        fireEvent.keyDown(editor, { key: "Enter" });
        expect(mocks.onSend).not.toHaveBeenCalled();
        expect((await screen.findByRole("button", { name: "worktree" })).title).toBe("Running Install…");
    });
});

describe("sending while dictating", () => {
    it("stops the mic and sends what was typed together with what was said", async () => {
        const pane = document.createElement("div");
        document.body.append(pane);
        const paneRef = { current: pane };
        setState({ voiceDictation: true });
        useVoice.setState({ phase: "ready", reason: null, stage: null, fraction: 0, target: null, partial: "" });
        const editor = renderComposer(undefined, paneRef);
        type(editor, "fix the");

        act(() => toggleDictation(pane));
        act(() => handleVoiceEvent({ type: "listening" }));
        fireEvent.click(screen.getByRole("button", { name: "Send message" }));
        expect(voice.stop).toHaveBeenCalledOnce();
        expect(mocks.onSend).not.toHaveBeenCalled();

        act(() => handleVoiceEvent({ type: "transcript", text: "login redirect" }));
        await waitFor(() => expect(mocks.onSend).toHaveBeenCalledWith({ text: "fix the login redirect", paths: [], context: [] }, false));
        pane.remove();
    });

    it("lets a message that is only spoken be sent", () => {
        const pane = document.createElement("div");
        document.body.append(pane);
        setState({ voiceDictation: true });
        useVoice.setState({ phase: "ready", reason: null, stage: null, fraction: 0, target: null, partial: "" });
        renderComposer(undefined, { current: pane });
        expect(screen.getByRole("button", { name: "Send message" })).toBeDisabled();
        act(() => toggleDictation(pane));
        act(() => handleVoiceEvent({ type: "listening" }));
        expect(screen.getByRole("button", { name: "Send message" })).toBeEnabled();
        pane.remove();
    });
});
