import { act, render, waitFor } from "@testing-library/react";
import { EditorView } from "@codemirror/view";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { emit } from "../state/bus";
import { getState, setState } from "../state/store";
import { useToasts } from "../state/toast";
import { EditorPane } from "./EditorPane";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("./EditorFindBar", () => ({ EditorFindBar: () => null }));

const initial = getState();
const path = "/repo/README.md";

Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
Range.prototype.getBoundingClientRect = () => new DOMRect();

describe("EditorPane external changes", () => {
    let content: string;
    let version: string;
    let failReads: boolean;

    beforeEach(() => {
        setState(initial, true);
        useToasts.setState({ toasts: [] });
        content = "disk version one";
        version = "version-1";
        failReads = false;
        invoke.mockReset();
        invoke.mockImplementation(async (command: string) => {
            if (command === "read_file_versioned") {
                if (failReads) throw { category: "io", message: "io: No such file or directory (os error 2)" };
                return { content, version };
            }
            return null;
        });
        setState({
            editorViews: { pane: { openTabs: [path], activePath: path } },
        });
    });

    it("keeps a clean buffer clean when a refresh read fails, then loads the next disk version", async () => {
        const { container } = render(<EditorPane paneId="pane" cwd="/repo" active visible showInsights={false} />);

        await waitFor(() => expect(container.querySelector(".cm-content")).toHaveTextContent("disk version one"));
        useToasts.setState({ toasts: [] });
        failReads = true;

        act(() => emit({ type: "fs-changed", repo: "/repo" }));
        await waitFor(() => expect(invoke.mock.calls.filter(([command]) => command === "read_file_versioned").length).toBeGreaterThan(2));

        expect(getState().dirtyEditorPaths.pane ?? []).toEqual([]);
        expect(container.querySelector(".tab-dot")).not.toBeInTheDocument();
        expect(useToasts.getState().toasts).toEqual([]);

        failReads = false;
        content = "disk version two";
        version = "version-2";
        act(() => emit({ type: "fs-changed", repo: "/repo" }));

        await waitFor(() => expect(container.querySelector(".cm-content")).toHaveTextContent("disk version two"));
        expect(getState().dirtyEditorPaths.pane ?? []).toEqual([]);
        expect(useToasts.getState().toasts).toEqual([]);
    });

    it("shows a conflict only when unsaved edits and a separate disk change coexist", async () => {
        const { container } = render(<EditorPane paneId="pane" cwd="/repo" active visible showInsights={false} />);

        await waitFor(() => expect(container.querySelector(".cm-content")).toHaveTextContent("disk version one"));
        const editor = EditorView.findFromDOM(container.querySelector(".cm-editor") as HTMLElement);
        expect(editor).not.toBeNull();
        act(() => editor!.dispatch({ changes: { from: editor!.state.doc.length, insert: " unsaved" } }));
        await waitFor(() => expect(getState().dirtyEditorPaths.pane).toEqual([path]));

        content = "separate disk version";
        version = "version-2";
        act(() => emit({ type: "fs-changed", repo: "/repo" }));

        await waitFor(() =>
            expect(useToasts.getState().toasts.some((toast) => toast.text.includes("changed while this editor had unsaved work"))).toBe(true),
        );
        expect(editor!.state.doc.toString()).toBe("disk version one unsaved");
    });
});
