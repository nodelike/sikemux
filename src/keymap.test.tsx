import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { loadApplicationActions } from "./actions/bridge";
import { browserApi } from "./api/browser";
import { IS_MACOS } from "./lib/platform";
import type { Session } from "./state/types";
import { getState, setState } from "./state/store";
import { useKeymap } from "./keymap";

const initial = getState();

function session(id: string, kind: Session["kind"] = "project"): Session {
    return {
        id,
        name: id,
        kind,
        cwd: `/tmp/${id}`,
        pinned: false,
        activeWindowId: `${id}-window`,
        activeAgentId: null,
        view: "windows",
    };
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

describe("Alt+Tab session switching", () => {
    it("previews each session and commits only when Alt is released", () => {
        render(<KeymapHarness />);

        window.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", code: "Tab", altKey: true, bubbles: true, cancelable: true }));
        expect(getState().activeSessionId).toBe("one");
        expect(getState().sessionSwitcher?.selectedSessionId).toBe("two");
        expect(getState().zoomedPaneId).toBe("zoomed");

        window.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", code: "Tab", altKey: true, bubbles: true, cancelable: true }));
        expect(getState().activeSessionId).toBe("one");
        expect(getState().sessionSwitcher?.selectedSessionId).toBe("three");

        window.dispatchEvent(new KeyboardEvent("keyup", { key: "Alt", code: "AltLeft", bubbles: true, cancelable: true }));
        expect(getState().activeSessionId).toBe("three");
        expect(getState().sessionSwitcher).toBeNull();
        expect(getState().zoomedPaneId).toBeNull();
    });

    it("cancels the preview with Escape", () => {
        render(<KeymapHarness />);

        window.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", code: "Tab", altKey: true, bubbles: true, cancelable: true }));
        window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", code: "Escape", altKey: true, bubbles: true, cancelable: true }));

        expect(getState().activeSessionId).toBe("one");
        expect(getState().sessionSwitcher).toBeNull();
    });
});

describe("agent picker shortcut", () => {
    it("opens the agent picker modal with Alt+N from the agent view", () => {
        setState((state) => ({
            sessions: { ...state.sessions, one: { ...state.sessions.one, view: "agent" } },
        }));
        render(<KeymapHarness />);

        window.dispatchEvent(new KeyboardEvent("keydown", { key: "n", code: "KeyN", altKey: true, bubbles: true, cancelable: true }));

        expect(getState().agentPaletteOpen).toBe(true);
        expect(getState().sessions.one.view).toBe("agent");
    });
});

describe("embedded browser shortcuts", () => {
    it("opens the new-tab chooser with Command+T rather than a browser tab", () => {
        const open = vi.spyOn(browserApi, "newTab").mockResolvedValue("browser-tab");
        setState((state) => ({
            sessions: { ...state.sessions, one: { ...state.sessions.one, view: "agent", activeAgentId: "agent-one" } },
        }));
        render(<KeymapHarness />);

        window.dispatchEvent(
            new KeyboardEvent("keydown", {
                code: "KeyT",
                metaKey: IS_MACOS,
                ctrlKey: !IS_MACOS,
                bubbles: true,
                cancelable: true,
            }),
        );

        expect(getState().newTabPaletteOpen).toBe(true);
        expect(open).not.toHaveBeenCalled();
        open.mockRestore();
    });

    it("opens a browser tab for the active agent with Command+Shift+T", async () => {
        const open = vi.spyOn(browserApi, "newTab").mockResolvedValue("browser-tab");
        setState((state) => ({
            sessions: { ...state.sessions, one: { ...state.sessions.one, view: "agent", activeAgentId: "agent-one" } },
        }));
        render(<KeymapHarness />);

        window.dispatchEvent(
            new KeyboardEvent("keydown", {
                code: "KeyT",
                shiftKey: true,
                metaKey: IS_MACOS,
                ctrlKey: !IS_MACOS,
                bubbles: true,
                cancelable: true,
            }),
        );

        await waitFor(() => expect(open).toHaveBeenCalledWith("agent-one"));
        open.mockRestore();
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

        window.dispatchEvent(new KeyboardEvent("keydown", { key: "z", code: "KeyZ", altKey: true, bubbles: true, cancelable: true }));
        expect(getState().zoomedPaneId).toBe("zoomed");
        expect(getState().commandPopup).not.toBeNull();

        window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", code: "Escape", bubbles: true, cancelable: true }));
        expect(getState().commandPopup).toBeNull();
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
                    keybinding: "Meta+Shift+KeyT",
                },
                {
                    id: "zoom-collision",
                    label: "Do not override zoom",
                    description: "Built-ins retain priority",
                    command: "echo no",
                    placement: "background",
                    contexts: ["project"],
                    keybinding: "Alt+KeyZ",
                },
            ],
            isCurrent: () => true,
            execute,
        });

        try {
            render(<KeymapHarness />);

            window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyZ", altKey: true, bubbles: true, cancelable: true }));
            expect(getState().zoomedPaneId).toBeNull();
            expect(execute).not.toHaveBeenCalled();

            window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyT", metaKey: true, shiftKey: true, bubbles: true, cancelable: true }));
            await waitFor(() => expect(execute).toHaveBeenCalledWith(expect.objectContaining({ id: "quality" })));
            expect(getState().recentCommandKeys.filter((key) => key === "standalone:project.action.quality")).toHaveLength(1);

            registration.dispose();
            execute.mockClear();
            window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyT", metaKey: true, shiftKey: true, bubbles: true, cancelable: true }));
            await Promise.resolve();
            expect(execute).not.toHaveBeenCalled();
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
