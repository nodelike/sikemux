import { bench, describe } from "vitest";
import { parseDiffFromFile, type FileContents } from "@pierre/diffs";
import { PerformanceTelemetry } from "./lib/performance";
import { rankBy } from "./lib/fuzzy";
import { computeLayout, splitPane } from "./state/layout";
import type { LayoutNode, PaneNode } from "./state/types";

const candidates = Array.from({ length: 5_000 }, (_, index) => `src/project-${index}/component-${index % 97}.tsx`);
const pane = (id: string): PaneNode => ({ type: "pane", id, cwd: "/repo", kind: "terminal", title: id });
let layout: LayoutNode = pane("pane-0");
for (let index = 1; index < 24; index += 1) {
    layout = splitPane(layout, `pane-${index - 1}`, index % 2 === 0 ? "row" : "column", pane(`pane-${index}`));
}

function pierreInputs(fileCount: number, lineCount: number, changeEvery: number): Array<readonly [FileContents, FileContents]> {
    return Array.from({ length: fileCount }, (_, fileIndex) => {
        const base: string[] = [];
        const head: string[] = [];
        for (let lineIndex = 0; lineIndex < lineCount; lineIndex += 1) {
            const line = `export const value_${fileIndex}_${lineIndex} = ${lineIndex};`;
            base.push(line);
            head.push(lineIndex % changeEvery === 0 ? `export const value_${fileIndex}_${lineIndex} = ${lineIndex + 1};` : line);
        }
        const name = `src/file-${fileIndex}.ts`;
        return [
            { name, contents: base.join("\n"), lang: "typescript", cacheKey: `base-${fileIndex}` },
            { name, contents: head.join("\n"), lang: "typescript", cacheKey: `head-${fileIndex}` },
        ] as const;
    });
}

const manyPierreDiffs = pierreInputs(1_000, 250, 25);
const tallPierreDiffs = pierreInputs(25, 2_000, 100);

describe("interactive hot paths", () => {
    bench("rank 5,000 file-palette candidates", () => {
        rankBy("component 42", candidates, (value) => value);
    });

    bench("compute a 24-pane layout", () => {
        computeLayout(layout);
    });

    bench("record 100 bounded telemetry samples", () => {
        const telemetry = new PerformanceTelemetry({ spanCapacity: 64, latencySampleCapacity: 64 });
        for (let index = 0; index < 100; index += 1) {
            const span = telemetry.startTrace("bench", { index });
            const recorded = telemetry.endSpan(span);
            if (recorded) telemetry.recordLatency("bench", recorded.durationMs);
        }
        telemetry.snapshot();
    });

    bench("parse 1,000 Pierre diffs with 10 changes each", () => {
        for (const [base, head] of manyPierreDiffs) parseDiffFromFile(base, head);
    });

    bench("parse 25 Pierre diffs with 2,000 lines each", () => {
        for (const [base, head] of tallPierreDiffs) parseDiffFromFile(base, head);
    });
});
