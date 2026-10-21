import { cleanup, fireEvent, render, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { EditorPane } from "./EditorPane";
import { getState, setState } from "../state/store";
import { fileRead, wholeFile } from "../test/files";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("./EditorFindBar", () => ({ EditorFindBar: () => null }));

const initial = getState();

function queueWithoutLine() {
    const [target] = getState().pendingEditorOpens.pane;
    setState({ pendingEditorOpens: { pane: [{ ...target, line: undefined, column: undefined }] } });
}

Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
Range.prototype.getBoundingClientRect = () => new DOMRect();

describe("EditorPane CLI queue", () => {
    afterEach(async () => {
        cleanup();
        // React can still have a render queued after unmounting; let it run while the page exists.
        await new Promise((resolve) => setTimeout(resolve, 0));
    });

    beforeEach(() => {
        setState(initial, true);
        invoke.mockReset();
        invoke.mockImplementation(async (command: string, args?: unknown) => {
            if (fileRead(command, args)) {
                return wholeFile("# Preview heading\n\n**locked**\n\n| Key | Action |\n| --- | --- |\n| ⌘P | Open project |", "version-1");
            }
            if (command === "cli_open_result") return undefined;
            return null;
        });
        setState({
            editorViews: { pane: { openTabs: [], activePath: null } },
            pendingEditorOpens: {
                pane: [
                    {
                        requestId: "request-1",
                        id: "target-1",
                        kind: "file",
                        path: "/repo/README.md",
                        projectRoot: "/repo",
                        line: 1,
                        column: 2,
                    },
                ],
            },
        });
    });

    it("loads, activates, and acknowledges a queued file", async () => {
        render(<EditorPane paneId="pane" cwd="/repo" active visible showInsights={false} />);

        await waitFor(() =>
            expect(invoke).toHaveBeenCalledWith("cli_open_result", {
                result: {
                    requestId: "request-1",
                    targetId: "target-1",
                    paneId: "pane",
                    path: "/repo/README.md",
                    error: null,
                },
            }),
        );
        expect(getState().editorViews.pane).toMatchObject({
            openTabs: ["/repo/README.md"],
            activePath: "/repo/README.md",
        });
        expect(getState().pendingEditorOpens).toEqual({});
    });

    it("opens Markdown in preview with source editing disabled, and switches to source", async () => {
        queueWithoutLine();
        const { container } = render(<EditorPane paneId="pane" cwd="/repo" active visible showInsights={false} />);
        const editor = within(container);

        expect(await editor.findByRole("heading", { name: "Preview heading" })).toBeInTheDocument();
        expect(editor.getByText("locked", { selector: "strong" })).toBeInTheDocument();
        expect(editor.getByRole("table")).toBeInTheDocument();
        expect(editor.getByRole("columnheader", { name: "Key" })).toBeInTheDocument();
        expect(editor.getByRole("cell", { name: "Open project" })).toBeInTheDocument();
        expect(container.querySelector(".ed-host")).toHaveClass("preview-mode");
        expect(container.querySelector(".ed-source-host")).toHaveAttribute("hidden");
        expect(container.querySelector(".ed-source-host")).toHaveAttribute("aria-hidden", "true");
        expect(container.querySelector(".cm-content")).toHaveAttribute("contenteditable", "false");

        fireEvent.click(editor.getByRole("button", { name: "Show source for README.md" }));

        expect(editor.queryByRole("heading", { name: "Preview heading" })).not.toBeInTheDocument();
        expect(container.querySelector(".ed-host")).not.toHaveClass("preview-mode");
        expect(container.querySelector(".ed-source-host")).not.toHaveAttribute("hidden");
        expect(container.querySelector(".ed-source-host")).toHaveAttribute("aria-hidden", "false");
        expect(container.querySelector(".cm-content")).toHaveAttribute("contenteditable", "true");
        expect(editor.getByRole("button", { name: "Preview README.md" }).querySelector("svg")).toBeInTheDocument();
    });

    it("opens preview links outside the app window, and relative ones as files in the editor", async () => {
        const readme = "[site](https://example.com/docs) [guide](./docs/My%20Guide.md#setup) [top](#top)";
        const fallback = invoke.getMockImplementation()!;
        invoke.mockImplementation(async (command: string, args?: unknown) => {
            if (fileRead(command, args)) return wholeFile(readme, "version-1");
            return fallback(command, args);
        });
        queueWithoutLine();
        const { container } = render(<EditorPane paneId="pane" cwd="/repo" active visible showInsights={false} />);
        const editor = within(container);
        expect(fireEvent.click(await editor.findByRole("link", { name: "site" }))).toBe(false);
        expect(invoke).toHaveBeenCalledWith("open_url", { url: "https://example.com/docs", app: null, shortcut: null });

        expect(fireEvent.click(editor.getByRole("link", { name: "top" }))).toBe(false);

        expect(fireEvent.click(editor.getByRole("link", { name: "guide" }))).toBe(false);
        await waitFor(() => expect(getState().editorViews.pane.activePath).toBe("/repo/docs/My Guide.md"));
    });
});
