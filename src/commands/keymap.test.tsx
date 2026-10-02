import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { loadApplicationActions } from "../actions/bridge";
import { agentApi } from "../api/agents";
import { browserApi } from "../api/browser";
import { IS_MACOS } from "../lib/platform";
import type { Session } from "../state/types";
import { getState, setState } from "../state/store";
import { useKeymap } from "./keymap";
import { getKeybindingAction, type CoreKeybindingActionId } from "./keybindings";
import { activeAgentId, agentWindowId } from "../state/selectors";
import { withAgents } from "../test/agents";

const initial = getState();

function session(id: string, kind: Session["kind"] = "project"): Session {
    return {
        id,
        name: id,
        kind,
        cwd: `/tmp/${id}`,
        pinned: false,
        activeWindowId: `${id}-window`,
    };
}

/** Puts an agent in `sessionId` and makes it the window on screen. */
function lookAtAgent(sessionId: string, agentId: string): void {
    setState((state) => {
        const slices = withAgents(state, sessionId, [{ id: agentId, type: "codex", title: agentId, startup: "codex", launchState: "live" }]);
        return {
            ...slices,
            sessions: { ...state.sessions, [sessionId]: { ...state.sessions[sessionId], activeWindowId: agentWindowId(slices, agentId)! } },
        };
    });
}

/** Presses whatever key `action` is bound to by default on this platform. */
function pressAction(action: CoreKeybindingActionId, target: EventTarget = window): void {
    const parts = (getKeybindingAction(action).defaultBinding ?? "").split("+");
    const code = parts.pop() ?? "";
    target.dispatchEvent(
        new KeyboardEvent("keydown", {
            code,
            metaKey: parts.includes("Meta"),
            ctrlKey: parts.includes("Ctrl"),
            altKey: parts.includes("Alt"),
            shiftKey: parts.includes("Shift"),
            bubbles: true,
            cancelable: true,
        }),
    );
}

function KeymapHarness() {
    useKeymap();
    return null;
}

function EditableKeymapHarness() {
    useKeymap();
    return <input aria-label="Editable target" />;
}

beforeEach(() => {
    setState(initial, true);
    setState({
        sessions: { one: session("one"), two: session("two"), three: session("three"), command: session("command", "command") },
        sessionOrder: ["one", "two", "three", "command"],
        activeSessionId: "one",
        sessionSwitcher: null,
        zoomedPaneId: "zoomed",
        keybindingOverrides: {},
    });
});

describe("Control+` session switching", () => {
    it("previews each session and commits only when Control is released", () => {
        render(<KeymapHarness />);

        pressAction("session.next");
        expect(getState().activeSessionId).toBe("one");
        expect(getState().sessionSwitcher?.selectedSessionId).toBe("two");
        expect(getState().zoomedPaneId).toBe("zoomed");

        pressAction("session.next");
        expect(getState().activeSessionId).toBe("one");
        expect(getState().sessionSwitcher?.selectedSessionId).toBe("three");

        window.dispatchEvent(new KeyboardEvent("keyup", { key: "Control", code: "ControlLeft", bubbles: true, cancelable: true }));
        expect(getState().activeSessionId).toBe("three");
        expect(getState().sessionSwitcher).toBeNull();
        expect(getState().zoomedPaneId).toBeNull();
    });

    it("cancels the preview with Escape", () => {
        render(<KeymapHarness />);

        pressAction("session.next");
        window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", code: "Escape", ctrlKey: true, bubbles: true, cancelable: true }));

        expect(getState().activeSessionId).toBe("one");
        expect(getState().sessionSwitcher).toBeNull();
    });
});

describe("agent shortcuts", () => {
    const codex = { type: "codex" as const, label: "Codex", command: "/bin/codex", available: true, defaultModel: null, defaultEffort: null };

    it("starts the agent launched last, from any tab of the project", async () => {
        const available = vi.spyOn(agentApi, "available").mockResolvedValue([{ ...codex, type: "claude", label: "Claude" }, codex]);
        setState({ lastAgentType: "codex" });
        render(<KeymapHarness />);

        pressAction("agent.new");

        await waitFor(() => expect(activeAgentId(getState(), getState().sessions.one)).not.toBeNull());
        const agentId = activeAgentId(getState(), getState().sessions.one)!;
        expect(getState().agents[agentId].type).toBe("codex");
        available.mockRestore();
    });

    it("asks for a project first when none is in front, then starts the agent there", async () => {
        const available = vi.spyOn(agentApi, "available").mockResolvedValue([codex]);
        setState({ activeSessionId: "command" });
        render(<KeymapHarness />);

        pressAction("agent.new");
        expect(getState()).toMatchObject({ pickerOpen: true, pickerMode: "projects" });

        setState({ activeSessionId: "two", pickerOpen: false });
        await waitFor(() => expect(activeAgentId(getState(), getState().sessions.two)).not.toBeNull());
        available.mockRestore();
    });

    it("opens the agent picker from a terminal tab too", () => {
        render(<KeymapHarness />);

        pressAction("agent.choose");

        expect(getState().agentPaletteOpen).toBe(true);
    });
});

describe("terminal and browser shortcuts", () => {
    it("opens a terminal tab with Command+T, not a chooser or a browser tab", () => {
        const open = vi.spyOn(browserApi, "newTab").mockResolvedValue("browser-tab");
        lookAtAgent("one", "agent-one");
        const before = getState().windowsBySession.one?.length ?? 0;
        render(<KeymapHarness />);

        pressAction("terminal.new");

        const windows = getState().windowsBySession.one ?? [];
        expect(windows).toHaveLength(before + 1);
        expect(getState().windows[getState().sessions.one.activeWindowId].role).toBe("term");
        expect(getState().newTabPaletteOpen).toBe(false);
        expect(open).not.toHaveBeenCalled();
        open.mockRestore();
    });

    it("opens a browser tab for the active agent", async () => {
        const open = vi.spyOn(browserApi, "newTab").mockResolvedValue("browser-tab");
        lookAtAgent("one", "agent-one");
        render(<KeymapHarness />);

        pressAction("browser.tabNew");

        await waitFor(() => expect(open).toHaveBeenCalledWith("agent-one"));
        open.mockRestore();
    });

    it("brings the project's agent forward for a browser tab asked for from a terminal", async () => {
        const open = vi.spyOn(browserApi, "newTab").mockResolvedValue("browser-tab");
        lookAtAgent("one", "agent-one");
        const agentWindow = getState().sessions.one.activeWindowId;
        setState((state) => ({ sessions: { ...state.sessions, one: { ...state.sessions.one, activeWindowId: "one-window" } } }));
        render(<KeymapHarness />);

        pressAction("browser.tabNew");

        await waitFor(() => expect(open).toHaveBeenCalledWith("agent-one"));
        expect(getState().sessions.one.activeWindowId).toBe(agentWindow);
        open.mockRestore();
    });
});

describe("Option in text", () => {
    function TerminalHarness() {
        useKeymap();
        return (
            <div className="xterm">
                <textarea aria-label="Terminal input" />
            </div>
        );
    }

    afterEach(cleanup);

    it("reaches the shell even when someone bound an Option chord", () => {
        setState({ keybindingOverrides: { "pane.zoom": "Alt+KeyZ" } });
        render(<TerminalHarness />);

        fireEvent.keyDown(screen.getByRole("textbox", { name: "Terminal input" }), { code: "KeyZ", key: "Ω", altKey: true });
        expect(getState().zoomedPaneId).toBe("zoomed");

        window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyZ", altKey: true, bubbles: true, cancelable: true }));
        expect(getState().zoomedPaneId).toBeNull();
    });
});

describe("command popup modality", () => {
    it("blocks workspace shortcuts and closes on Escape", () => {
        render(<KeymapHarness />);
        setState({
            commandPopup: {
                id: "popup-1",
                title: "Logs",
                startup: "tail -f app.log",
                cwd: "/tmp",
                context: { sessionId: "one", sessionName: "one", sessionKind: "command" },
            },
        });

        pressAction("pane.zoom");
        expect(getState().zoomedPaneId).toBe("zoomed");
        expect(getState().commandPopup).not.toBeNull();

        window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", code: "Escape", bubbles: true, cancelable: true }));
        expect(getState().commandPopup).toBeNull();
    });
});

describe("shortcuts while settings is open", () => {
    it("leaves settings for the place the shortcut goes", () => {
        render(<KeymapHarness />);
        setState({ settingsOpen: true });

        pressAction("session.lastUsed");
        pressAction("window.agents");

        expect(getState().settingsOpen).toBe(false);
    });

    it("closes settings instead of the pane hidden behind it", () => {
        render(<KeymapHarness />);
        setState({ settingsOpen: true });

        pressAction("pane.close");

        expect(getState().settingsOpen).toBe(false);
        expect(getState().sessions.one).toBeTruthy();
    });

    it("keeps settings open for the command palette", () => {
        render(<KeymapHarness />);
        setState({ settingsOpen: true });

        pressAction("palette.commands");

        expect(getState()).toMatchObject({ settingsOpen: true, commandPaletteOpen: true });
    });
});

describe("onboarding modality", () => {
    it("leaves every binding to the tour, including the ones other modals let through", () => {
        render(<KeymapHarness />);
        setState({ onboardingOpen: true });

        // palette.commands escapes the usual modal guard; the first-run tour asks
        // the reader to press it, so nothing behind the tour may react.
        window.dispatchEvent(
            new KeyboardEvent("keydown", { key: "P", code: "KeyP", metaKey: true, shiftKey: true, bubbles: true, cancelable: true }),
        );
        window.dispatchEvent(new KeyboardEvent("keydown", { key: ",", code: "Comma", metaKey: true, bubbles: true, cancelable: true }));
        window.dispatchEvent(new KeyboardEvent("keydown", { key: "s", code: "KeyS", altKey: true, bubbles: true, cancelable: true }));

        expect(getState()).toMatchObject({ commandPaletteOpen: false, settingsOpen: false, pickerOpen: false, onboardingOpen: true });
    });
});

describe("contributed action keybindings", () => {
    it("dispatches contextual project bindings, revokes them, and keeps built-ins first", async () => {
        const execute = vi.fn();
        const runtime = await loadApplicationActions();
        const registration = runtime.registerProjectActions({
            projectId: "one",
            projectRoot: "/tmp/one",
            configPath: "/tmp/one/sikemux.json",
            actions: [
                {
                    id: "quality",
                    label: "Run quality checks",
                    description: "Lint and test",
                    command: "pnpm check",
                    placement: "terminal",
                    contexts: ["project"],
                    keybinding: "Meta+Alt+KeyQ",
                },
                {
                    id: "zoom-collision",
                    label: "Do not override zoom",
                    description: "Built-ins retain priority",
                    command: "echo no",
                    placement: "background",
                    contexts: ["project"],
                    keybinding: getKeybindingAction("pane.zoom").defaultBinding!,
                },
            ],
            isCurrent: () => true,
            execute,
        });

        try {
            render(<KeymapHarness />);

            pressAction("pane.zoom");
            expect(getState().zoomedPaneId).toBeNull();
            expect(execute).not.toHaveBeenCalled();

            window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyQ", metaKey: true, altKey: true, bubbles: true, cancelable: true }));
            await waitFor(() => expect(execute).toHaveBeenCalledWith(expect.objectContaining({ id: "quality" })));
            expect(getState().recentCommandKeys.filter((key) => key === "standalone:project.action.quality")).toHaveLength(1);

            registration.dispose();
            execute.mockClear();
            window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyQ", metaKey: true, altKey: true, bubbles: true, cancelable: true }));
            await Promise.resolve();
            expect(execute).not.toHaveBeenCalled();
        } finally {
            registration.dispose();
        }
    });

    it("runs a project action on a built-in's key when the built-in has nothing to act on", async () => {
        const execute = vi.fn();
        const runtime = await loadApplicationActions();
        const registration = runtime.registerProjectActions({
            projectId: "one",
            projectRoot: "/tmp/one",
            configPath: "/tmp/one/sikemux.json",
            actions: [
                {
                    id: "yank",
                    label: "Shares a key with agent permissions",
                    description: "",
                    command: "echo",
                    placement: "background",
                    contexts: ["project"],
                    keybinding: getKeybindingAction("agent.permissions").defaultBinding!,
                },
            ],
            isCurrent: () => true,
            execute,
        });

        try {
            render(<KeymapHarness />);
            pressAction("agent.permissions");
            await waitFor(() => expect(execute).toHaveBeenCalledWith(expect.objectContaining({ id: "yank" })));
        } finally {
            registration.dispose();
        }
    });

    it("does not let a Shift-only trusted action capture editable typing", async () => {
        const run = vi.fn();
        const runtime = await loadApplicationActions();
        const registration = runtime.register({
            id: "sikemux.editable-actions",
            actions: [
                {
                    id: "uppercase-a",
                    create: () => ({
                        commandId: "test.uppercase-a",
                        definition: {
                            id: "test.uppercaseA",
                            title: "Uppercase A",
                            detail: "Must not capture typing",
                            category: "Test",
                            source: "test.actions",
                            defaultBinding: "Shift+KeyA",
                            run,
                        },
                    }),
                },
            ],
        });

        try {
            render(<EditableKeymapHarness />);
            fireEvent.keyDown(screen.getByRole("textbox", { name: "Editable target" }), {
                code: "KeyA",
                key: "A",
                shiftKey: true,
            });
            await Promise.resolve();
            expect(run).not.toHaveBeenCalled();
        } finally {
            registration.dispose();
        }
    });
});

describe("text size shortcuts", () => {
    const MOD = IS_MACOS ? { metaKey: true } : { ctrlKey: true };

    // The harnesses in this block render real nodes, unlike the null-rendering
    // ones above, so each case has to start from an empty document.
    afterEach(cleanup);

    function ChatHarness() {
        useKeymap();
        return (
            <div className="agent-chat-pane">
                <button aria-label="In chat" />
            </div>
        );
    }

    function press(code: string, target: EventTarget, extra: Record<string, boolean> = {}) {
        target.dispatchEvent(new KeyboardEvent("keydown", { key: code, code, ...MOD, ...extra, bubbles: true, cancelable: true }));
    }

    beforeEach(() => {
        setState({ terminalFontSize: 13, chatTextScale: 1, editorTextScale: 1 });
    });

    it("resizes the terminal when the press lands outside a chat", () => {
        render(<KeymapHarness />);

        press("Equal", window);

        expect(getState().terminalFontSize).toBe(14);
        expect(getState().chatTextScale).toBe(1);
    });

    function EditorHarness() {
        useKeymap();
        return (
            <div className="cm-editor">
                <button aria-label="In editor" />
            </div>
        );
    }

    it("resizes the editor when the press lands inside one", () => {
        render(<EditorHarness />);

        press("Equal", screen.getByRole("button", { name: "In editor" }));

        expect(getState().editorTextScale).toBe(1.1);
        expect(getState().terminalFontSize).toBe(13);
        expect(getState().chatTextScale).toBe(1);
    });

    it("resizes the transcript when the press lands inside a chat", () => {
        render(<ChatHarness />);

        press("Equal", screen.getByRole("button", { name: "In chat" }));

        expect(getState().chatTextScale).toBe(1.1);
        expect(getState().terminalFontSize).toBe(13);
    });

    it("shrinks and resets the transcript it is focused in", () => {
        render(<ChatHarness />);
        const inChat = screen.getByRole("button", { name: "In chat" });

        press("Minus", inChat);
        expect(getState().chatTextScale).toBe(0.9);

        press("Digit0", inChat);
        expect(getState().chatTextScale).toBe(1);
    });

    it("resizes the active pane's transcript when focus has fallen to the page", () => {
        setState((state) => ({
            windows: { ...state.windows, "one-window": { ...state.windows["one-window"], id: "one-window", activePaneId: "chat-pane" } },
        }));
        function PaneHarness() {
            useKeymap();
            return (
                <div data-pane-id="chat-pane">
                    <div className="agent-chat-pane" />
                </div>
            );
        }
        render(<PaneHarness />);

        press("Equal", document.body);

        expect(getState().chatTextScale).toBe(1.1);
        expect(getState().terminalFontSize).toBe(13);
    });
});
