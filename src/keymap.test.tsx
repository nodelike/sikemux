import { render } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
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
