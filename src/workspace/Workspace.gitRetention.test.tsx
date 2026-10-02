import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useEffect } from "react";
import { retainWorkbenchWindows, Workspace } from "./Workspace";
import * as cmd from "../state/commands";
import { getState, setState } from "../state/store";

const lifecycle = vi.hoisted(() => ({ mounted: vi.fn(), unmounted: vi.fn(), editorMounted: vi.fn(), editorUnmounted: vi.fn() }));
vi.mock("../git/GitPane", () => ({
    GitPane: ({ active }: { active: boolean }) => {
        useEffect(() => {
            lifecycle.mounted();
            return lifecycle.unmounted;
        }, []);
        return <div data-testid="git-workbench" data-active={active} />;
    },
}));
vi.mock("../editor/EditorPane", () => ({
    EditorPane: ({ active }: { active: boolean }) => {
        useEffect(() => {
            lifecycle.editorMounted();
            return lifecycle.editorUnmounted;
        }, []);
        return <textarea data-testid="file-editor" data-active={active} defaultValue="unsaved buffer" />;
    },
}));
vi.mock("../terminal/TerminalPane", () => ({
    TerminalPane: ({ context }: { context?: { paneId?: string } }) => (
        <textarea data-testid={`terminal-${context?.paneId}`} defaultValue="shell output" />
    ),
}));
const initial = getState();
beforeEach(() => {
    vi.clearAllMocks();
    setState(initial, true);
    cmd.createProjectSession("/work/demo");
});
afterEach(cleanup);

it("retains a visited terminal across switches and releases its layer on close", async () => {
    cmd.newWindow();
    const state = getState();
    const terminalWindow = state.sessions[state.activeSessionId].activeWindowId;
    const terminalId = `terminal-${state.windows[terminalWindow].activePaneId}`;
    const { getByTestId, queryByTestId } = render(<Workspace />);
    await waitFor(() => expect(getByTestId(terminalId)).toBeInTheDocument());
    const terminal = getByTestId(terminalId);
    for (let index = 0; index < 10; index++) {
        act(() => cmd.openGitWorkbench());
        expect(getByTestId(terminalId)).toBe(terminal);
        expect(terminal.closest('[role="tabpanel"]')).toHaveAttribute("inert");
        act(() => cmd.selectTab({ id: terminalWindow }));
        expect(getByTestId(terminalId)).toBe(terminal);
        expect(terminal.closest('[role="tabpanel"]')).not.toHaveAttribute("inert");
    }
    act(() => cmd.closeWindowById(terminalWindow));
    expect(queryByTestId(terminalId)).toBeNull();
});

it("keeps Git mounted but inactive between tab switches, and releases it when closed", async () => {
    cmd.openGitWorkbench();
    const state = getState();
    const gitWindow = state.sessions[state.activeSessionId].activeWindowId;
    const { getByTestId } = render(<Workspace />);
    await waitFor(() => expect(lifecycle.mounted).toHaveBeenCalledTimes(1));
    for (let i = 0; i < 3; i++) {
        act(() => cmd.selectWindowByRole("term"));
        expect(getByTestId("git-workbench")).toHaveAttribute("data-active", "false");
        expect(getByTestId("git-workbench").closest('[role="tabpanel"]')).toHaveAttribute("inert");
        act(() => cmd.openGitWorkbench());
        expect(getByTestId("git-workbench")).toHaveAttribute("data-active", "true");
    }
    expect(lifecycle.mounted).toHaveBeenCalledTimes(1);
    expect(lifecycle.unmounted).not.toHaveBeenCalled();
    act(() => cmd.closeWindowById(gitWindow));
    expect(lifecycle.unmounted).toHaveBeenCalledTimes(1);
});

it("retains the editor and its local buffer across Git switches, then releases it on close", async () => {
    cmd.requestOpenFile("/work/demo/file.ts");
    const state = getState();
    const editorWindow = state.sessions[state.activeSessionId].activeWindowId;
    const { getByTestId } = render(<Workspace />);
    await waitFor(() => expect(lifecycle.editorMounted).toHaveBeenCalledTimes(1));
    const editor = getByTestId("file-editor") as HTMLTextAreaElement;
    editor.value = "edited text that has not been saved";
    editor.setSelectionRange(7, 11);
    for (let i = 0; i < 10; i++) {
        act(() => cmd.openGitWorkbench());
        expect(getByTestId("file-editor")).toBe(editor);
        expect(editor).toHaveAttribute("data-active", "false");
        expect(editor.closest('[role="tabpanel"]')).toHaveAttribute("inert");
        await waitFor(() => expect(lifecycle.mounted).toHaveBeenCalledTimes(1));
        act(() => cmd.requestOpenFile("/work/demo/file.ts"));
        expect(getByTestId("file-editor")).toBe(editor);
        expect(editor).toHaveAttribute("data-active", "true");
        expect(editor.value).toBe("edited text that has not been saved");
        expect([editor.selectionStart, editor.selectionEnd]).toEqual([7, 11]);
    }
    expect(lifecycle.editorMounted).toHaveBeenCalledTimes(1);
    expect(lifecycle.editorUnmounted).not.toHaveBeenCalled();
    act(() => cmd.closeWindowById(editorWindow));
    expect(lifecycle.editorUnmounted).toHaveBeenCalledTimes(1);
});

describe("retainWorkbenchWindows", () => {
    const alive = () => true;

    it("keeps the most recently visited screens and drops the oldest", () => {
        const retained = new Set<string>();
        for (const id of ["a", "b", "c", "d"]) retainWorkbenchWindows(retained, id, alive, 3);
        expect([...retained]).toEqual(["b", "c", "d"]);
    });

    it("moves a revisited screen back to the front of the queue", () => {
        const retained = new Set(["a", "b", "c"]);
        retainWorkbenchWindows(retained, "a", alive, 3);
        retainWorkbenchWindows(retained, "d", alive, 3);
        expect([...retained]).toEqual(["c", "a", "d"]);
    });

    it("releases a screen whose window is gone", () => {
        const retained = new Set(["a", "b"]);
        retainWorkbenchWindows(retained, null, (id) => id !== "a");
        expect([...retained]).toEqual(["b"]);
    });
});
