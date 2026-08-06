import { beforeEach, describe, expect, it } from "vitest";
import { consumeCliEditorOpen, createProjectSession, routeCliOpenRequest } from "./commands";
import { collectPanes } from "./layout";
import { getState, setState } from "./store";
import type { CliOpenRequest } from "./types";

const initial = getState();

beforeEach(() => setState(initial, true));

function request(path: string, projectRoot: string, kind: "file" | "directory" = "file"): CliOpenRequest {
    return {
        id: "request-1",
        cwd: projectRoot,
        wait: kind === "file",
        targets: [{ id: "target-1", kind, path, projectRoot, line: kind === "file" ? 4 : undefined, column: kind === "file" ? 2 : undefined }],
    };
}

describe("CLI open routing", () => {
    it("creates a project, selects its files window, and queues a file for the editor", () => {
        const result = routeCliOpenRequest(request("/repo/src/main.ts", "/repo"));

        expect(result).toEqual([]);
        const st = getState();
        const session = st.sessions[st.activeSessionId];
        expect(session).toMatchObject({ kind: "project", cwd: "/repo", view: "windows" });
        const win = st.windows[session.activeWindowId];
        expect(win.role).toBe("files");
        const pane = collectPanes(win.root).find((candidate) => candidate.kind === "editor");
        expect(pane).toBeDefined();
        expect(st.pendingEditorOpens[pane!.id]).toEqual([
            {
                id: "target-1",
                kind: "file",
                path: "/repo/src/main.ts",
                projectRoot: "/repo",
                line: 4,
                column: 2,
                requestId: "request-1",
            },
        ]);
    });

    it("routes to the deepest existing project that owns the target", () => {
        createProjectSession("/workspace");
        createProjectSession("/workspace/packages/app");

        routeCliOpenRequest(request("/workspace/packages/app/src/index.ts", "/workspace"));

        expect(getState().sessions[getState().activeSessionId].cwd).toBe("/workspace/packages/app");
    });

    it("routes an external file to an explicit project root", () => {
        routeCliOpenRequest(request("/tmp/generated.patch", "/workspace"));

        expect(getState().sessions[getState().activeSessionId].cwd).toBe("/workspace");
        expect(Object.values(getState().pendingEditorOpens).flat()).toEqual([
            expect.objectContaining({ path: "/tmp/generated.patch", projectRoot: "/workspace" }),
        ]);
    });

    it("acknowledges a directory immediately without adding editor work", () => {
        const result = routeCliOpenRequest(request("/repo", "/repo", "directory"));

        expect(result).toEqual([
            {
                requestId: "request-1",
                targetId: "target-1",
                paneId: null,
                path: "/repo",
                error: null,
            },
        ]);
        expect(getState().pendingEditorOpens).toEqual({});
    });

    it("consumes only the completed queue item", () => {
        routeCliOpenRequest(request("/repo/src/main.ts", "/repo"));
        const [paneId] = Object.keys(getState().pendingEditorOpens);

        consumeCliEditorOpen(paneId, "request-1", "target-1");

        expect(getState().pendingEditorOpens).toEqual({});
    });
});
