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
    it("shows the half of the pane it will take, on the edge nearest the pointer, then splits there", () => {
        cmd.newWindow();
        const [first, second] = getState().windowsBySession[getState().activeSessionId];
        const paneOf = (id: string) => collectPanes(getState().windows[id].root)[0].id;
        const [firstPane, secondPane] = [paneOf(first), paneOf(second)];
        const { container } = render(<Workspace />);
        const area = container.querySelector<HTMLElement>(".window-area")!;
        vi.spyOn(area, "getBoundingClientRect").mockReturnValue({ left: 0, right: 1000, top: 0, bottom: 800, width: 1000, height: 800 } as DOMRect);
        const target = container.querySelector<HTMLElement>(`[data-pane-id="${secondPane}"]`)!;
        vi.spyOn(target, "getBoundingClientRect").mockReturnValue({
            left: 0,
            right: 1000,
            top: 40,
            bottom: 800,
            width: 1000,
            height: 760,
        } as DOMRect);
        document.elementsFromPoint = () => [target];
        const firstTab = screen.getAllByRole("tab")[0];
        const preview = () => container.querySelector<HTMLElement>(".split-preview");

        fireEvent.pointerDown(firstTab, { button: 0, clientX: 10, clientY: 10 });
        fireEvent.pointerMove(window, { clientX: 20, clientY: 300 });
        expect(preview()).toHaveClass("split-preview--left");
        expect(preview()?.style.width).toBe("500px");
        fireEvent.pointerMove(window, { clientX: 500, clientY: 790 });
        expect(preview()).toHaveClass("split-preview--bottom");
        expect(preview()?.style.top).toBe("420px");
        fireEvent.pointerMove(window, { clientX: 980, clientY: 300 });
        expect(preview()).toHaveClass("split-preview--right");

        fireEvent.pointerUp(window, { clientX: 980, clientY: 300 });
        act(() => vi.advanceTimersByTime(TAB_SLIDE_MS));

        expect(container.querySelector(".split-preview")).toBeNull();
        expect(getState().windowsBySession[getState().activeSessionId]).toEqual([second]);
        expect(collectPanes(shown().root).map((pane) => pane.id)).toEqual([secondPane, firstPane]);
        expect(first in getState().windows).toBe(false);
    });
});

describe("the divider between split panes", () => {
    const sizes = () => {
        const root = shown().root;
        return root.type === "split" ? root.sizes : [];
    };
    const divider = () => screen.getByRole("separator");

    beforeEach(() => {
        HTMLElement.prototype.setPointerCapture = vi.fn();
        cmd.splitActivePane("row");
    });

    it("takes focus when grabbed, so the arrow keys carry on moving it", () => {
        render(<Workspace />);

        fireEvent.pointerDown(divider(), { button: 0, pointerId: 1, clientX: 500, clientY: 100 });
        expect(divider()).toHaveFocus();
        expect(divider()).toHaveClass("dragging");
        fireEvent.pointerUp(divider(), { pointerId: 1 });
        expect(divider()).not.toHaveClass("dragging");

        fireEvent.keyDown(divider(), { key: "ArrowRight" });
        expect(sizes()[0]).toBeCloseTo(0.52);
    });

    it("evens the panes out on a double-click", () => {
        render(<Workspace />);
        const split = shown().root;
        if (split.type === "split") cmd.setSplitSizes(shown().id, split.id, [0.8, 0.2]);

        fireEvent.doubleClick(divider());

        expect(sizes()).toEqual([0.5, 0.5]);
    });
});
