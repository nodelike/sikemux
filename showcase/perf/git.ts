import type { DiffRow, GitStatus } from "../../src/api/git";
import type { ShowcaseBackend } from "../backend";
import {
  SESSION,
  everyFrame,
  frames,
  jank,
  quantile,
  restingFrame,
  showcase,
  sleep,
  spread,
} from "./measure";

/*
 * Times the git screen the way a person meets it: opening it, switching to
 * and from it, and scrolling its review, on a repository with as many dirty
 * files as asked for.
 */

const GIT = "w-sikemux-git";
const OTHERS = ["w-sikemux-term", "w-sikemux-files"];

async function scroll(by: number, count: number, from: number) {
  const list = document.querySelector<HTMLElement>(".merge-review-list");
  if (!list) throw new Error("the review list is not on screen");
  list.scrollTop = from;
  await sleep(600);
  return spread(await everyFrame(count, () => (list.scrollTop += by)));
}

/** Diffs built from the files themselves, a line in seven added and one in nine removed, so they colour like real code. */
function serveDiffs(backend: ShowcaseBackend): void {
  backend.on("git_file_diff", async ({ repo, path }) => {
    const response = await fetch("/__showcase/read_file", {
      method: "POST",
      body: JSON.stringify({ path: `${repo}/${path}` }),
    });
    const text: unknown = response.ok ? await response.json() : "";
    const rows: DiffRow[] = [];
    (typeof text === "string" ? text : "")
      .split("\n")
      .forEach((line, index) => {
        if (index % 9 === 4) rows.push([2, index + 1, `${line} // before`]);
        rows.push([index % 7 === 3 ? 1 : 0, index + 1, line]);
      });
    return rows;
  });
}

async function dirty(count: number): Promise<void> {
  const { GIT_STATUS } = await import("../world/git");
  const repo = Object.keys(GIT_STATUS)[0];
  const response = await fetch("/__showcase/list_files", {
    method: "POST",
    body: JSON.stringify({ repo }),
  });
  const files = ((await response.json()) as string[])
    .filter((path) => /\.(tsx?|rs|css|md)$/.test(path))
    .slice(0, count);
  const status: GitStatus = GIT_STATUS[repo];
  status.files = files.map((path, index) => ({
    path,
    index: index % 5 === 0 ? "M" : " ",
    worktree: index % 5 === 0 ? " " : "M",
  }));
}

export async function git({
  dirtyFiles = 1500,
  switches = 3,
}: { dirtyFiles?: number; switches?: number } = {}) {
  const { backend, cmd } = showcase();
  serveDiffs(backend);
  await dirty(dirtyFiles);
  cmd.selectSession(SESSION);
  cmd.selectWindowId(OTHERS[0]);
  await sleep(1000);

  const frameMs = await restingFrame();
  const jankOf = (lengths: number[]) => jank(lengths, frameMs);
  const open = jankOf(await frames(3000, () => cmd.selectWindowId(GIT)));
  const switching: ReturnType<typeof jank>[] = [];
  for (let round = 0; round < switches; round++) {
    for (const other of OTHERS) {
      switching.push(
        jankOf(await frames(1000, () => cmd.selectWindowId(other))),
      );
      switching.push(jankOf(await frames(1000, () => cmd.selectWindowId(GIT))));
    }
  }
  return {
    dirtyFiles,
    frameMs,
    open,
    switch: {
      longestMs: quantile(
        switching.map((one) => one.longestMs),
        0.5,
      ),
      worstMs: Math.max(...switching.map((one) => one.longestMs)),
    },
    scroll: await scroll(40, 240, 0),
    fling: await scroll(150, 240, 100_000),
  };
}
