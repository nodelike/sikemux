import type { DiffRow, GitFile, GitStatus } from "../../src/api/git";
import { FRONT, MOODBOARD, SIKEMUX } from "./projects";

const staged = (path: string, index = "M"): GitFile => ({
  path,
  index,
  worktree: " ",
});
const modified = (path: string): GitFile => ({
  path,
  index: " ",
  worktree: "M",
});
const untracked = (path: string): GitFile => ({
  path,
  index: "?",
  worktree: "?",
});

export const GIT_STATUS: Record<string, GitStatus> = {
  [SIKEMUX]: {
    branch: "main",
    upstream: "origin/main",
    ahead: 2,
    behind: 0,
    files: [
      staged("src/styles/modern-shell.css"),
      staged("src/rail/TabRailDensity.test.tsx", "A"),
      modified("src/rail/TabRail.tsx"),
      modified("src/styles/tokens.css"),
      modified("DESIGN.md"),
      untracked("src/rail/TabRailDensity.tsx"),
    ],
  },
  [FRONT]: {
    branch: "main",
    upstream: "origin/main",
    ahead: 0,
    behind: 0,
    files: [
      modified("src/components/Hero.astro"),
      modified("src/components/DownloadButton.astro"),
    ],
  },
  [MOODBOARD]: {
    branch: "palette-kmeans",
    upstream: "origin/palette-kmeans",
    ahead: 1,
    behind: 0,
    files: [modified("benchmarks/palette.py")],
  },
};

export const COMMIT_DRAFT: Record<string, string> = {
  [SIKEMUX]:
    "fix(sidebar): line up agent names with project names\n\nIn the compact layout the agent names sat 2px to the right of the project names above them. Both now use the same spacing.",
};

export const BRANCHES: Record<string, string[]> = {
  [SIKEMUX]: ["main", "release/0.4", "rail-density"],
  [FRONT]: ["main"],
  [MOODBOARD]: ["main", "palette-kmeans"],
};

const context = (line: number, text: string): DiffRow => [0, line, text];
const added = (line: number, text: string): DiffRow => [1, line, text];
const deleted = (line: number, text: string): DiffRow => [2, line, text];
const hidden = (lines: number): DiffRow => [3, lines, ""];

/** The changes behind the commit draft, so the review shows what the commit says. */
export const FILE_DIFFS: Record<string, DiffRow[]> = {
  "src/styles/modern-shell.css": [
    hidden(361),
    context(362, ".agent-row {"),
    deleted(363, "    padding-left: calc(var(--row-inset) + 2px);"),
    added(363, "    padding-left: var(--row-inset);"),
    context(364, "    padding-right: var(--row-inset);"),
    context(365, "}"),
    hidden(40),
    context(406, ".proj-row,"),
    context(407, ".agent-row {"),
    added(408, "    --row-inset: var(--space-1);"),
    context(409, "    gap: var(--space-2);"),
    context(410, "}"),
  ],
  "src/rail/TabRailDensity.test.tsx": [
    added(1, 'import { render } from "@testing-library/react";'),
    added(2, 'import { describe, expect, it } from "vitest";'),
    added(3, 'import { TabRailDensity } from "./TabRailDensity";'),
    added(4, ""),
    added(5, 'describe("compact rail", () => {'),
    added(6, '    it("lines agent names up with project names", () => {'),
    added(
      7,
      "        const { container } = render(<TabRailDensity compact />);",
    ),
    added(
      8,
      '        expect(container.firstElementChild).toHaveAttribute("data-density", "compact");',
    ),
    added(9, "    });"),
    added(10, "});"),
  ],
  "src/rail/TabRail.tsx": [
    hidden(82),
    context(83, "    return ("),
    deleted(
      84,
      '        <aside className="workspace-rail tab-rail" data-density={density}>',
    ),
    added(84, '        <TabRailDensity compact={density === "compact"}>'),
    context(85, "            {header}"),
    context(86, "            {list}"),
    deleted(87, "        </aside>"),
    added(87, "        </TabRailDensity>"),
    context(88, "    );"),
  ],
  "src/styles/tokens.css": [
    hidden(214),
    context(
      215,
      "    /* Rows in the rails share one inset, so their marks line up down the rail. */",
    ),
    deleted(216, "    --row-inset: 6px;"),
    added(216, "    --row-inset: var(--space-1);"),
    context(217, "    --row-slot: 22px;"),
  ],
  "DESIGN.md": [
    hidden(118),
    context(119, "## Rails"),
    context(120, ""),
    deleted(121, "Agent rows sit a little inside project rows."),
    added(
      121,
      "Agent rows and project rows share one inset, so names line up down the rail.",
    ),
    context(122, ""),
  ],
  "src/rail/TabRailDensity.tsx": [
    added(1, 'import type { ReactNode } from "react";'),
    added(2, ""),
    added(3, "/** The tab rail, at the density the person chose. */"),
    added(
      4,
      "export function TabRailDensity({ compact, children }: { compact?: boolean; children?: ReactNode }) {",
    ),
    added(5, "    return ("),
    added(
      6,
      '        <aside className="workspace-rail tab-rail" data-density={compact ? "compact" : "comfortable"}>',
    ),
    added(7, "            {children}"),
    added(8, "        </aside>"),
    added(9, "    );"),
    added(10, "}"),
  ],
};
