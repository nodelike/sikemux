import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { emit } from "../state/bus";
import { getState, setState } from "../state/store";
import { EditorPane } from "./EditorPane";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({
    invoke,
    convertFileSrc: (path: string, protocol: string) => `${protocol}://localhost/${encodeURIComponent(path)}`,
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("./EditorFindBar", () => ({ EditorFindBar: () => null }));

const initial = getState();
const url = (path: string) => `preview://localhost/${encodeURIComponent(path)}`;

Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
Range.prototype.getBoundingClientRect = () => new DOMRect();

describe("EditorPane viewers", () => {
    let files: Record<string, { mime: string; size: number; modified: number }>;

    beforeEach(() => {
        setState(initial, true);
        files = {};
        invoke.mockReset();
        invoke.mockImplementation(async (command: string, args: { path: string }) => {
            if (command === "preview_file") return files[args.path];
            if (command === "read_file_versioned") {
                if (files[args.path]) throw { category: "not-text", message: `${args.path} is not UTF-8 text` };
                return { content: "text", version: "v1" };
            }
            return null;
        });
        vi.stubGlobal(
            "fetch",
            vi.fn(async () => new Response(new Uint8Array([0x7f, 0x45, 0x4c, 0x46, 0x02, 0x01]))),
        );
    });

    afterEach(() => {
        cleanup();
        vi.unstubAllGlobals();
    });

    const open = (path: string) => {
        setState({ editorViews: { pane: { openTabs: [path], activePath: path } } });
        return render(<EditorPane paneId="pane" cwd="/repo" active visible showInsights={false} />);
    };

    it("shows a PDF from the preview scheme without reading it as text", async () => {
        files["/repo/spec.pdf"] = { mime: "application/pdf", size: 2048, modified: 1 };
        open("/repo/spec.pdf");

        await waitFor(() => expect(screen.getByTitle("spec.pdf")).toHaveAttribute("src", expect.stringContaining(url("/repo/spec.pdf"))));
        expect(invoke.mock.calls.some(([command]) => command === "read_file_versioned")).toBe(false);
    });

    it("plays audio in a media element", async () => {
        files["/repo/take.m4a"] = { mime: "audio/mp4", size: 4096, modified: 1 };
        const { container } = open("/repo/take.m4a");

        await waitFor(() => expect(container.querySelector("audio")).toHaveAttribute("src", expect.stringContaining(url("/repo/take.m4a"))));
    });

    it("falls back to a hex view when a file with an unknown name is not text", async () => {
        files["/repo/tool"] = { mime: "application/octet-stream", size: 6, modified: 1 };
        const { container } = open("/repo/tool");

        await waitFor(() => expect(container.querySelector(".ed-viewer-hex")).toHaveTextContent("00000000 7f 45 4c 46 02 01"));
        expect(getState().editorViews.pane?.openTabs).toEqual(["/repo/tool"]);
    });

    it("loads the file again only when it changed on disk", async () => {
        files["/repo/shot.png"] = { mime: "image/png", size: 10, modified: 1 };
        open("/repo/shot.png");
        await waitFor(() => expect(screen.getByRole("img", { name: "shot.png" })).toBeInTheDocument());
        const first = screen.getByRole("img", { name: "shot.png" }).getAttribute("src");

        act(() => emit({ type: "fs-changed", repo: "/repo" }));
        await waitFor(() => expect(invoke.mock.calls.filter(([command]) => command === "preview_file").length).toBeGreaterThan(1));
        expect(screen.getByRole("img", { name: "shot.png" }).getAttribute("src")).toBe(first);

        files["/repo/shot.png"] = { mime: "image/png", size: 12, modified: 2 };
        act(() => emit({ type: "fs-changed", repo: "/repo" }));
        await waitFor(() => expect(screen.getByRole("img", { name: "shot.png" }).getAttribute("src")).not.toBe(first));
    });
});
