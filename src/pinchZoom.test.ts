import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installPinchZoom, PINCH_IDLE_MS } from "./pinchZoom";
import { getState, setState } from "./state/store";

const initial = getState();
let uninstall: () => void;

function surface(className: string): HTMLElement {
    const outer = document.createElement("div");
    outer.className = className;
    const inner = document.createElement("span");
    outer.append(inner);
    document.body.append(outer);
    return inner;
}

function gesture(target: Element, type: string, scale: number): Event {
    const event = new Event(type, { bubbles: true, cancelable: true });
    Object.assign(event, { scale });
    target.dispatchEvent(event);
    return event;
}

function pinchWheel(target: Element, deltaY: number, ctrlKey = true): WheelEvent {
    const event = new WheelEvent("wheel", { bubbles: true, cancelable: true, deltaY, ctrlKey });
    target.dispatchEvent(event);
    return event;
}

beforeEach(() => {
    vi.useFakeTimers();
    setState(initial, true);
    setState({ chatTextScale: 1, editorTextScale: 1, terminalFontSize: 13 });
    uninstall = installPinchZoom();
});

afterEach(() => {
    uninstall();
    document.body.innerHTML = "";
    vi.useRealTimers();
});

describe("pinching to resize text", () => {
    it("scales the chat under the pointer from where the pinch began", () => {
        const chat = surface("agent-chat-pane");
        gesture(chat, "gesturestart", 1);
        gesture(chat, "gesturechange", 1.2);
        expect(getState().chatTextScale).toBe(1.2);
        gesture(chat, "gesturechange", 1.5);
        expect(getState().chatTextScale).toBe(1.5);
        gesture(chat, "gestureend", 1.5);
        expect(getState().editorTextScale).toBe(1);
    });

    it("resizes the editor and terminal each on their own", () => {
        const editor = surface("cm-editor");
        gesture(editor, "gesturestart", 1);
        gesture(editor, "gesturechange", 0.8);
        gesture(editor, "gestureend", 0.8);
        const terminal = surface("terminal-shell");
        gesture(terminal, "gesturestart", 1);
        gesture(terminal, "gesturechange", 2);
        gesture(terminal, "gestureend", 2);
        expect(getState().editorTextScale).toBe(0.8);
        expect(getState().terminalFontSize).toBe(26);
        expect(getState().chatTextScale).toBe(1);
    });

    it("keeps the page from zooming only when a text surface takes the pinch", () => {
        const chat = surface("agent-chat-pane");
        expect(gesture(chat, "gesturestart", 1).defaultPrevented).toBe(true);
        const elsewhere = surface("side-rail");
        gesture(chat, "gestureend", 1);
        expect(gesture(elsewhere, "gesturestart", 1).defaultPrevented).toBe(false);
        expect(pinchWheel(elsewhere, -50).defaultPrevented).toBe(false);
    });

    it("adds up small ctrl-wheel steps so the terminal still grows", () => {
        const terminal = surface("terminal-shell");
        for (let i = 0; i < 10; i += 1) pinchWheel(terminal, -10);
        expect(getState().terminalFontSize).toBe(26);
    });

    it("starts a new pinch after the wheel goes quiet", () => {
        const chat = surface("agent-chat-pane");
        pinchWheel(chat, -100);
        expect(getState().chatTextScale).toBe(2);
        vi.advanceTimersByTime(PINCH_IDLE_MS);
        setState({ chatTextScale: 1 });
        pinchWheel(chat, 100);
        expect(getState().chatTextScale).toBe(0.7);
    });

    it("leaves ordinary scrolling alone", () => {
        const chat = surface("agent-chat-pane");
        expect(pinchWheel(chat, 40, false).defaultPrevented).toBe(false);
        expect(getState().chatTextScale).toBe(1);
    });
});
