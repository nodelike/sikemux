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
      staged("src/components/AgentRailDensity.test.tsx", "A"),
      modified("src/components/AgentRail.tsx"),
      modified("src/styles/tokens.css"),
      modified("DESIGN.md"),
      untracked("src/components/AgentRailDensity.tsx"),
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
    "fix(rail): rows keep one inset and one gap at every density\n\nThe compact density shaved the leading inset but not the gap after the mark, so labels drifted 2px right of the project rows above them.",
};

export const BRANCHES: Record<string, string[]> = {
  [SIKEMUX]: ["main", "release/0.4", "rail-density"],
  [FRONT]: ["main"],
  [MOODBOARD]: ["main", "palette-kmeans"],
};
