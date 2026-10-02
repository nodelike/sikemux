import type { GitFile, GitStatus } from "../../src/api/git";
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
      staged("src/rail/AgentRailDensity.test.tsx", "A"),
      modified("src/rail/AgentRail.tsx"),
      modified("src/styles/tokens.css"),
      modified("DESIGN.md"),
      untracked("src/rail/AgentRailDensity.tsx"),
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
