import { beforeEach, describe, expect, it, vi } from "vitest";
import { fsapi } from "../api/fs";
import { DocumentIO } from "../editor/documentIO";
import { subscribe } from "./bus";
import { renameEditorPath } from "./editorPaths";
import { getState, setState } from "./store";
vi.mock("../api/fs", () => ({ fsapi: { rename: vi.fn(), readFileVersioned: vi.fn(), writeFileVersioned: vi.fn() } }));
beforeEach(() => {
    vi.clearAllMocks();
    setState({
        editorViews: { a: { openTabs: ["/repo/src/a.ts", "/repo/src2/b.ts"], activePath: "/repo/src/a.ts" } },
        dirtyEditorPaths: { a: ["/repo/src/a.ts"] },
    });
});
describe("renaming open editor files", () => {
    it("moves nested tabs and dirty paths only after a successful rename", async () => {
        vi.mocked(fsapi.rename).mockResolvedValue(undefined);
        const onRename = vi.fn();
        const unsubscribe = subscribe("path-renamed", onRename);
        await renameEditorPath("/repo/src", "/repo/lib");
        unsubscribe();
        expect(getState().editorViews.a).toEqual({ openTabs: ["/repo/lib/a.ts", "/repo/src2/b.ts"], activePath: "/repo/lib/a.ts" });
        expect(getState().dirtyEditorPaths.a).toEqual(["/repo/lib/a.ts"]);
        expect(onRename).toHaveBeenCalledWith({ type: "path-renamed", src: "/repo/src", dest: "/repo/lib" });
    });
    it("leaves editor paths unchanged when the filesystem rejects the rename", async () => {
        vi.mocked(fsapi.rename).mockRejectedValue(new Error("Permission denied"));
        await expect(renameEditorPath("/repo/src", "/repo/lib")).rejects.toThrow("Permission denied");
        expect(getState().editorViews.a.activePath).toBe("/repo/src/a.ts");
    });
    it("uses the observed version when saving the relocated buffer", async () => {
        vi.mocked(fsapi.readFileVersioned).mockResolvedValue({ content: "saved", version: "v1" });
        vi.mocked(fsapi.writeFileVersioned).mockResolvedValue({ version: "v2" });
        const io = new DocumentIO();
        await io.read("/repo/a.ts");
        io.relocate("/repo/a.ts", "/repo/b.ts");
        await io.save("/repo/b.ts", "unsaved buffer");
        expect(fsapi.writeFileVersioned).toHaveBeenCalledWith("/repo/b.ts", "unsaved buffer", "v1");
        expect(io.version("/repo/a.ts")).toBeUndefined();
    });
});
