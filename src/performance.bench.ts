import { describe, test } from "vitest";
import type { DiffRow } from "./api/git";
import { layoutRows } from "./components/DiffView";
import { UiActivityTracker } from "./lib/activity";
import { PerformanceTelemetry } from "./lib/performance";
import { rankBy } from "./lib/fuzzy";
import { computeLayout, splitPane } from "./state/layout";
import { tokenizeCode } from "./chat/shikiTokens";
import { codeThemeName } from "./themes/codeTheme";
import { DEFAULT_THEME_ID, themeById } from "./themes";
import type { LayoutNode, PaneNode } from "./state/types";

const candidates = Array.from({ length: 5_000 }, (_, index) => `src/project-${index}/component-${index % 97}.tsx`);
const pane = (id: string): PaneNode => ({ type: "pane", id, cwd: "/repo", kind: "terminal", title: id });
let layout: LayoutNode = pane("pane-0");
for (let index = 1; index < 24; index += 1) {
    layout = splitPane(layout, `pane-${index - 1}`, index % 2 === 0 ? "row" : "column", pane(`pane-${index}`));
}

function diffRows(fileCount: number, lineCount: number): DiffRow[][] {
    return Array.from({ length: fileCount }, (_, fileIndex) =>
        Array.from({ length: lineCount }, (_, lineIndex): DiffRow => [
            lineIndex % 3 === 0 ? 1 : 0,
            lineIndex + 1,
            `export const value_${fileIndex}_${lineIndex} = ${lineIndex};`,
        ]),
    );
}

function measure(name: string, fn: () => unknown) {
    test(name, async ({ bench }) => {
        await bench(name, fn).run();
    });
}

const activityTracker = new UiActivityTracker();
for (let index = 0; index < 24; index += 1) activityTracker.beginCommand(`inflight_${index}`);

const manyDiffs = diffRows(1_000, 90);
const tallDiffs = diffRows(25, 2_000);

for (const count of [5_000, 50_000, 250_000]) {
    const files = Array.from({ length: count }, (_, index) => `src/project-${index % 97}/component-${index}.tsx`);
    test(`file palette with ${count.toLocaleString("en-US")} files`, async ({ bench }) => {
        await bench.compare(
            bench("rank all matches then slice 200", () => {
                rankBy("component", files, (path) => path).slice(0, 200);
            }),
            bench("retain only the best 200 matches", () => {
                rankBy("component", files, (path) => path, 200);
            }),
        );
    });
}

describe("interactive hot paths", () => {
    measure("rank 5,000 file-palette candidates", () => {
        rankBy("component 42", candidates, (value) => value);
    });

    measure("compute a 24-pane layout", () => {
        computeLayout(layout);
    });

    measure("record 100 bounded telemetry samples", () => {
        const telemetry = new PerformanceTelemetry({ spanCapacity: 64, latencySampleCapacity: 64 });
        for (let index = 0; index < 100; index += 1) {
            const span = telemetry.startTrace("bench", { index });
            const recorded = telemetry.endSpan(span);
            if (recorded) telemetry.recordLatency("bench", recorded.durationMs);
        }
        telemetry.snapshot();
    });

    measure("record 1,000 IPC calls in the activity tracker", () => {
        for (let index = 0; index < 1_000; index += 1) {
            activityTracker.endCommand(activityTracker.beginCommand("pty_write"), true);
        }
    });

    measure("build one activity report", () => {
        activityTracker.snapshot();
    });

    measure("lay out 1,000 diffs of 90 rows each", () => {
        for (const rows of manyDiffs) layoutRows(rows);
    });

    measure("lay out 25 diffs of 2,000 rows each", () => {
        for (const rows of tallDiffs) layoutRows(rows);
    });
});

/* What a chat fence costs to colour. The transcript asks for one fence at a
   time, once each, after it has stopped changing. */
const chatTheme = themeById(DEFAULT_THEME_ID);
const chatThemeName = codeThemeName(chatTheme);
const fence = (lines: number) => Array.from({ length: lines }, (_, line) => `export const value_${line} = fn(${line}); // note`).join("\n");
const shortFence = fence(20);
const longestFence = fence(150);

// Loaded once here so the runs below measure the reading rather than the
// grammar arriving, which a pane pays for at most once a session.
await tokenizeCode("const ready = true;", "typescript", chatTheme, chatThemeName);

describe("chat code fences", () => {
    measure("colour a 20-line typescript fence", async () => {
        await tokenizeCode(shortFence, "typescript", chatTheme, chatThemeName);
    });

    measure("colour the longest fence a chat will colour", async () => {
        await tokenizeCode(longestFence, "typescript", chatTheme, chatThemeName);
    });
});
