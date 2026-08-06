import { render, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { EditorPane } from "./EditorPane";
import { getState, setState } from "../state/store";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("./EditorFindBar", () => ({ EditorFindBar: () => null }));

const initial = getState();

Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
Range.prototype.getBoundingClientRect = () => new DOMRect();

describe("EditorPane CLI queue", () => {
    beforeEach(() => {
        setState(initial, true);
        invoke.mockReset();
        invoke.mockImplementation(async (command: string) => {
            if (command === "read_file") return "first\nsecond\nthird";
            if (command === "cli_open_result") return undefined;
            if (command === "repo_watch_start") return 1;
            return null;
        });
        setState({
            editorViews: { pane: { openTabs: [], activePath: null, treeWidth: 210 } },
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
        render(<EditorPane paneId="pane" cwd="/repo" active visible showTree={false} />);

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
});
