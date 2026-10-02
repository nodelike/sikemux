import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Annotation } from "../api";

const api = vi.hoisted(() => ({ annotations: vi.fn() }));
const editor = vi.hoisted(() => ({ requestOpenFile: vi.fn() }));

vi.mock("../../state/commands", async (importOriginal) => ({ ...(await importOriginal<object>()), requestOpenFile: editor.requestOpenFile }));

import { invalidate } from "../../plugin-api/resources";
import { LocalRepoProvider } from "../localRepo";
import { InHost, registerTestHost, TEST_HOST } from "../testHost";
import { annotationPlace, Annotations } from "./Annotations";

const host = registerTestHost(api);
const repo = { provider: TEST_HOST, owner: "nodelike", name: "sikemux" };

const note = (overrides: Partial<Annotation> = {}): Annotation => ({
    path: "src/app.ts",
    startLine: 4,
    endLine: 4,
    level: "failure",
    title: null,
    message: "boom",
    details: null,
    ...overrides,
});

async function renderAnnotations(found: Annotation[], cwd: string | null = null) {
    api.annotations.mockResolvedValue(found);
    const view = render(
        <InHost host={host}>
            <LocalRepoProvider value={cwd}>
                <Annotations repo={repo} checkRunId="c1" active />
            </LocalRepoProvider>
        </InHost>,
    );
    await act(async () => {});
    return view;
}

beforeEach(() => {
    invalidate(() => true);
    api.annotations.mockReset();
    editor.requestOpenFile.mockClear();
});

afterEach(cleanup);

describe("annotationPlace", () => {
    it("names a file, at a line when there is one", () => {
        expect(annotationPlace("src/app.ts", 4)).toBe("src/app.ts:4");
        expect(annotationPlace("src/app.ts", null)).toBe("src/app.ts");
        expect(annotationPlace(null, 4)).toBeNull();
    });
});

describe("Annotations", () => {
    it("shows nothing when CI flagged nothing", async () => {
        const view = await renderAnnotations([]);
        expect(view.container.textContent).toBe("");
    });

    it("colours failures and warnings, and leaves notices plain", async () => {
        await renderAnnotations([
            note({ level: "failure", message: "a" }),
            note({ level: "warning", message: "b" }),
            note({ level: "notice", message: "c" }),
        ]);
        const tones = [...document.querySelectorAll(".gha-callout")].map((callout) => callout.getAttribute("data-tone"));
        expect(tones).toEqual(["danger", "warn", null]);
    });

    it("shows a title only when CI gave one", async () => {
        await renderAnnotations([note({ title: "Type error", message: "a" }), note({ message: "b" })]);
        expect(document.querySelectorAll(".gha-annotation-title")).toHaveLength(1);
        expect(screen.getByText("Type error")).toBeTruthy();
    });

    it("names the place as plain text when the files are not checked out here", async () => {
        await renderAnnotations([note(), note({ path: null, message: "nowhere" })]);
        expect(screen.getByText("src/app.ts:4").tagName).toBe("SPAN");
        expect(screen.queryByRole("button")).toBeNull();
        expect(document.querySelectorAll(".gha-annotation-place")).toHaveLength(1);
    });

    it("opens the flagged file in the editor, at its line, when the files are checked out here", async () => {
        await renderAnnotations([note(), note({ path: "README.md", startLine: null, message: "b" })], "/code/sikemux");
        fireEvent.click(screen.getByRole("button", { name: "src/app.ts:4" }));
        expect(editor.requestOpenFile).toHaveBeenLastCalledWith("/code/sikemux/src/app.ts", 4);
        fireEvent.click(screen.getByRole("button", { name: "README.md" }));
        expect(editor.requestOpenFile).toHaveBeenLastCalledWith("/code/sikemux/README.md", undefined);
    });
});
