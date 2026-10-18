import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DiffRow } from "../api/git";
import { DiffView, layoutRows } from "./DiffView";

vi.mock("./diffColours", () => ({
    finishedColours: () => undefined,
    colourLines: (lines: readonly string[], _lang: string, _theme: unknown, _name: string, _max: number, listener: (colours: unknown) => void) => {
        listener(lines.map((text) => [{ text, color: "#123456" }]));
        return () => {};
    },
}));

afterEach(cleanup);

describe("DiffView", () => {
    it("measures rows, the widest line and the gutter's digits", () => {
        const rows: DiffRow[] = [
            [0, 9, "a\tb"],
            [3, 40, ""],
            [1, 1204, "abcdef"],
        ];
        const layout = layoutRows(rows);
        expect([...layout.offsets]).toEqual([0, 19, 23, 42]);
        expect(layout.columns).toBe(6);
        expect(layout.digits).toBe(4);
    });

    it("keeps only the rows near the viewport of a long diff in the document", () => {
        const rows: DiffRow[] = Array.from({ length: 5000 }, (_, index): DiffRow => [1, index + 1, `line ${index}`]);
        const { container } = render(
            <div style={{ height: 400, overflowY: "auto" }}>
                <DiffView rows={rows} path="notes.txt" tinted={false} />
            </div>,
        );
        const drawn = container.querySelectorAll(".diff-row").length;
        expect(drawn).toBeGreaterThan(0);
        expect(drawn).toBeLessThan(200);
        expect(container.querySelector<HTMLElement>(".diff-rows")!.style.height).toBe(`${5000 * 19}px`);
    });
});
