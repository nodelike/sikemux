import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../state/commands";
import { collectPanes } from "../state/layout";
import { getState, setState } from "../state/store";
import type { LayoutNode, PaneNode } from "../state/types";
import { Workspace, WorkspaceTabs } from "./Workspace";
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
        const { container } = render(
            <>
                <WorkspaceTabs />
                <Workspace />
            </>,
        );
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
        render(
            <>
                <WorkspaceTabs />
                <Workspace />
            </>,
        );

        fireEvent.pointerDown(divider(), { button: 0, pointerId: 1, clientX: 500, clientY: 100 });
        expect(divider()).toHaveFocus();
        expect(divider()).toHaveClass("dragging");
        fireEvent.pointerUp(divider(), { pointerId: 1 });
        expect(divider()).not.toHaveClass("dragging");

        fireEvent.keyDown(divider(), { key: "ArrowRight" });
        expect(sizes()[0]).toBeCloseTo(0.52);
    });

    it("evens the panes out on a double-click", () => {
        render(
            <>
                <WorkspaceTabs />
                <Workspace />
            </>,
        );
        const split = shown().root;
        if (split.type === "split") cmd.setSplitSizes(shown().id, split.id, [0.8, 0.2]);

        fireEvent.doubleClick(divider());

        expect(sizes()).toEqual([0.5, 0.5]);
    });
});

describe("moving a pane back to the tab bar", () => {
    it("offers a button on each terminal of a split tab that puts it back where its tab was", () => {
        cmd.newWindow();
        const [first, second] = getState().windowsBySession[getState().activeSessionId];
        cmd.splitWithTab(getState().activeSessionId, { id: first }, "left");
        render(
            <>
                <WorkspaceTabs />
                <Workspace />
            </>,
        );

        const buttons = screen.getAllByRole("button", { name: "Move back to the tab bar" });
        expect(buttons).toHaveLength(2);
        fireEvent.click(buttons[0]);

        expect(getState().windowsBySession[getState().activeSessionId]).toEqual([first, second]);
        expect(screen.queryByRole("button", { name: "Move back to the tab bar" })).toBeNull();
    });
});

describe("a split tab in the strip", () => {
    it("shows each pane as a tab, outlined together, and focuses the one clicked", () => {
        cmd.newWindow();
        const [first, second] = getState().windowsBySession[getState().activeSessionId];
        const secondPane = collectPanes(getState().windows[second].root)[0].id;
        cmd.splitWithTab(getState().activeSessionId, { id: first }, "left");
        const { container } = render(<WorkspaceTabs />);

        const group = container.querySelector(".tab-group")!;
        const members = group.querySelectorAll("[role='tab']");
        expect(members).toHaveLength(2);
        expect(Array.from(members).filter((tab) => tab.getAttribute("aria-selected") === "true")).toHaveLength(1);

        const unfocused = Array.from(members).find((tab) => tab.getAttribute("aria-selected") === "false")!;
        fireEvent.click(unfocused);

        expect(shown().activePaneId).toBe(secondPane);
        expect(unfocused).toHaveAttribute("aria-selected", "true");
    });

    it("closes only the pane whose tab's close is pressed", () => {
        cmd.newWindow();
        const [first, second] = getState().windowsBySession[getState().activeSessionId];
        const firstPane = collectPanes(getState().windows[first].root)[0].id;
        cmd.splitWithTab(getState().activeSessionId, { id: first }, "left");
        const { container } = render(<WorkspaceTabs />);

        const closes = container.querySelectorAll<HTMLButtonElement>(".tab-group .tab-x");
        expect(closes).toHaveLength(2);
        fireEvent.click(closes[0]);

        expect(getState().windowsBySession[getState().activeSessionId]).toEqual([second]);
        expect(collectPanes(shown().root).map((pane) => pane.id)).not.toContain(firstPane);
        expect(collectPanes(shown().root)).toHaveLength(1);
        expect(container.querySelector(".tab-group")).toBeNull();
    });
});

describe("dragging a pane's tab out of its split", () => {
    const placePills = () =>
        document.querySelectorAll<HTMLElement>(".tab-wrap").forEach((wrap) => {
            const left = Number(wrap.dataset.index) * 100;
            vi.spyOn(wrap, "getBoundingClientRect").mockReturnValue({
                left,
                right: left + 100,
                top: 0,
                bottom: 30,
                width: 100,
                height: 30,
            } as DOMRect);
        });
    const drag = (tab: HTMLElement, from: number, to: number) => {
        fireEvent.pointerDown(tab, { button: 0, clientX: from, clientY: 10 });
        fireEvent.pointerMove(window, { clientX: to, clientY: 10 });
        fireEvent.pointerUp(window, { clientX: to, clientY: 10 });
        act(() => vi.advanceTimersByTime(TAB_SLIDE_MS));
    };

    it("gives the pane its own tab where it is dropped", () => {
        cmd.newWindow();
        cmd.newWindow();
        cmd.newWindow();
        const [first, second, third, fourth] = getState().windowsBySession[getState().activeSessionId];
        const fourthPane = collectPanes(getState().windows[fourth].root)[0].id;
        cmd.splitWithTab(getState().activeSessionId, { id: first }, "left");
        render(<WorkspaceTabs />);
        placePills();

        drag(screen.getAllByRole("tab")[3], 310, 110);

        const [a, moved, c, d] = getState().windowsBySession[getState().activeSessionId];
        expect([a, c, d]).toEqual([second, third, fourth]);
        expect(collectPanes(getState().windows[moved].root).map((pane) => pane.id)).toEqual([fourthPane]);
        expect(collectPanes(getState().windows[fourth].root)).toHaveLength(1);
    });

    it("stays in its split when dropped among the split's own tabs", () => {
        cmd.newWindow();
        cmd.newWindow();
        const [first, second, third] = getState().windowsBySession[getState().activeSessionId];
        cmd.splitWithTab(getState().activeSessionId, { id: first }, "left");
        render(<WorkspaceTabs />);
        placePills();

        drag(screen.getAllByRole("tab")[2], 210, 110);

        expect(getState().windowsBySession[getState().activeSessionId]).toEqual([second, third]);
        expect(collectPanes(getState().windows[third].root)).toHaveLength(2);
    });
});

describe("an agent's button back to the tab bar", () => {
    const showAgent = (...others: PaneNode[]) => {
        const sessionId = getState().activeSessionId;
        const agent: PaneNode = { type: "pane", id: "a1", cwd: "/code", kind: "agent", title: "codex" };
        const children = [agent, ...others];
        const root: LayoutNode =
            children.length > 1 ? { type: "split", id: "split", dir: "row", children, sizes: children.map(() => 1 / children.length) } : agent;
        setState(
            (state) =>
                ({
                    windows: { agent: { id: "agent", name: "codex", role: "agent", root, activePaneId: "a1" } },
                    windowsBySession: { [sessionId]: ["agent"] },
                    agents: { a1: { id: "a1", type: "codex", title: "codex" } },
                    sessions: { ...state.sessions, [sessionId]: { ...state.sessions[sessionId], activeWindowId: "agent" } },
                }) as never,
        );
        cmd.openDesk("a1");
        render(<Workspace />);
    };

    it("stays off while the agent only has its desk beside it", () => {
        showAgent();

        expect(collectPanes(shown().root).map((pane) => pane.kind)).toEqual(["agent", "desk"]);
        expect(screen.queryByRole("button", { name: "Move back to the tab bar" })).toBeNull();
    });

    it("shows on the agent and on the tab grouped with it", () => {
        showAgent({ type: "pane", id: "p1", cwd: "/code", kind: "terminal", title: "zsh" });

        expect(screen.getAllByRole("button", { name: "Move back to the tab bar" })).toHaveLength(2);
    });
});
