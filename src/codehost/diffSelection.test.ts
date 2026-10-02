import { describe, expect, it } from "vitest";
import { DIFF_ROW, type DiffRow } from "../api/git";
import { diffLinesDelivery, pickedRows } from "./diffSelection";

const pull = { number: 42, title: "Retry uploads", url: "https://github.com/o/r/pull/42" };

describe("diffLinesDelivery", () => {
    it("names the pull request, the file and both sides' lines, then the lines as a diff", () => {
        const rows: DiffRow[] = [
            [DIFF_ROW.context, 10, "const a = 1;"],
            [DIFF_ROW.deleted, 11, "retry(0);"],
            [DIFF_ROW.added, 11, "retry(3);"],
            [DIFF_ROW.hidden, 5, ""],
        ];
        expect(diffLinesDelivery(pull, "src/upload.ts", rows).text).toBe(
            'From pull request #42 "Retry uploads" (https://github.com/o/r/pull/42), src/upload.ts, old line 11 and new lines 10–11:\n\n' +
                "```diff\n const a = 1;\n-retry(0);\n+retry(3);\n```\n",
        );
    });

    it("names only the side the lines are on", () => {
        const text = diffLinesDelivery(pull, "a.ts", [[DIFF_ROW.added, 3, "x"]]).text ?? "";
        expect(text).toContain(", a.ts, new line 3:");
    });
});

describe("pickedRows", () => {
    function diff(): HTMLElement {
        const host = document.createElement("div");
        host.innerHTML = '<div data-row="0"><span>a</span></div><div data-row="1"><span>b</span></div><div data-row="2"><span>c</span></div>';
        document.body.append(host);
        return host;
    }

    it("spans the rows a text selection runs across, either way round", () => {
        const host = diff();
        const spans = host.querySelectorAll("span");
        const selection = window.getSelection()!;
        selection.setBaseAndExtent(spans[2].firstChild!, 1, spans[0].firstChild!, 0);
        expect(pickedRows(host, selection, null)).toEqual([0, 2]);
        selection.removeAllRanges();
        host.remove();
    });

    it("falls back to the row clicked when nothing is selected", () => {
        const host = diff();
        expect(pickedRows(host, window.getSelection(), host.querySelectorAll("span")[1])).toEqual([1, 1]);
        expect(pickedRows(host, null, host)).toBeNull();
        host.remove();
    });
});
