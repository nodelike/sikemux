import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DiffRow } from "../api/git";
import { DiffView, layoutRows } from "./DiffView";

vi.mock("../chat/shikiTokens", () => ({
    tokenizeLines: async (lines: readonly string[]) => lines.map((text) => [{ text, color: "#123456" }]),
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

    it("colours each row by the side it came from", async () => {
        const rows: DiffRow[] = [
            [0, 1, "let a = 1;"],
            [2, 2, "let b = 1;"],
            [1, 2, "let b = 2;"],
        ];
        const { container, findAllByText } = render(<DiffView rows={rows} path="src/a.ts" tinted />);
        const [coloured] = await findAllByText("let b = 1;", { selector: "span[style]" });
        expect(coloured).toHaveStyle({ color: "#123456" });
        expect(container.querySelectorAll(".diff-code span[style]")).toHaveLength(3);
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
        const list = container.querySelector<HTMLElement>(".diff-rows")!;
        expect(parseFloat(list.style.paddingTop) + parseFloat(list.style.paddingBottom) + drawn * 19).toBe(5000 * 19);
    });
});
