import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../state/commands";
import { collectPanes } from "../state/layout";
import { getState, setState } from "../state/store";
import { Workspace } from "./Workspace";
import { TAB_SLIDE_MS } from "./tabDrag";

vi.mock("../terminal/TerminalPane", () => ({ TerminalPane: () => <div /> }));

const initial = getState();
const shown = () => getState().windows[getState().sessions[getState().activeSessionId].activeWindowId];

beforeEach(() => {
    vi.useFakeTimers();
    setState(initial, true);
});
afterEach(() => {
    cleanup();
    vi.useRealTimers();
    vi.restoreAllMocks();
});

describe("dragging a tab onto the screen", () => {
    it("shows the half it will take, then splits it beside the tab on screen", () => {
        cmd.newWindow();
        const [first, second] = getState().windowsBySession[getState().activeSessionId];
        const paneOf = (id: string) => collectPanes(getState().windows[id].root)[0].id;
        const [firstPane, secondPane] = [paneOf(first), paneOf(second)];
        const { container } = render(<Workspace />);
        const area = container.querySelector<HTMLElement>(".window-area")!;
        vi.spyOn(area, "getBoundingClientRect").mockReturnValue({ left: 0, right: 1000, top: 0, bottom: 800, width: 1000, height: 800 } as DOMRect);
        const firstTab = screen.getAllByRole("tab")[0];

        fireEvent.pointerDown(firstTab, { button: 0, clientX: 10, clientY: 10 });
        fireEvent.pointerMove(window, { clientX: 20, clientY: 300 });
        expect(container.querySelector(".split-preview")).toHaveClass("split-preview--left");
        fireEvent.pointerMove(window, { clientX: 800, clientY: 300 });
        expect(container.querySelector(".split-preview")).toHaveClass("split-preview--right");

        fireEvent.pointerUp(window, { clientX: 800, clientY: 300 });
        act(() => vi.advanceTimersByTime(TAB_SLIDE_MS));

        expect(container.querySelector(".split-preview")).toBeNull();
        expect(getState().windowsBySession[getState().activeSessionId]).toEqual([second]);
        expect(collectPanes(shown().root).map((pane) => pane.id)).toEqual([secondPane, firstPane]);
        expect(first in getState().windows).toBe(false);
    });
});
