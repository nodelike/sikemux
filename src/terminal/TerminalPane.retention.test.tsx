import { render, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { TerminalPane, TERMINAL_RENDERER_RETENTION } from "./TerminalPane";

const { mounted } = vi.hoisted(() => ({ mounted: new Map<string, boolean>() }));
vi.mock("./usePty", () => ({
    usePty: ({ context }: { context?: { paneId?: string } }) => ({ current: { testPaneId: context?.paneId } }),
}));
vi.mock("./useXterm", () => ({
    useXterm: ({ ptyController, shouldMount }: { ptyController: { current: { testPaneId?: string } }; shouldMount: boolean }) => {
        mounted.set(ptyController.current.testPaneId ?? "", shouldMount);
        return {
            find: vi.fn(),
            clearSearch: vi.fn(),
            getSelection: vi.fn(() => ""),
            copySelection: vi.fn(),
            pasteClipboard: vi.fn(),
            selectAll: vi.fn(),
            copyScrollback: vi.fn(),
            clear: vi.fn(),
            focus: vi.fn(),
        };
    },
}));

function panes(visible: number[]) {
    return Array.from({ length: 5 }, (_, index) => {
        const paneId = `pane-${index}`;
        return (
            <TerminalPane
                key={paneId}
                active={visible.includes(index)}
                visible={visible.includes(index)}
                context={{ sessionId: "session", sessionName: "Project", sessionKind: "project", paneId }}
            />
        );
    });
}

it("keeps the three most recently hidden terminal renderers warm", async () => {
    expect(TERMINAL_RENDERER_RETENTION).toEqual({ keepaliveMs: 15_000, maxHidden: 3 });
    const view = render(<>{panes([0, 1, 2, 3, 4])}</>);
    view.rerender(<>{panes([4])}</>);

    await waitFor(() => expect(mounted.get("pane-0")).toBe(false));
    expect(mounted.get("pane-1")).toBe(true);
    expect(mounted.get("pane-2")).toBe(true);
    expect(mounted.get("pane-3")).toBe(true);
    expect(mounted.get("pane-4")).toBe(true);
});
