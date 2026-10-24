import type { ShowcaseBackend } from "../backend";

export interface Showcase {
  backend: ShowcaseBackend;
  cmd: {
    selectSession(id: string): void;
    selectWindowId(id: string): void;
    setThemeId(id: string): void;
  };
}

export const showcase = () =>
  (window as unknown as { showcase: Showcase }).showcase;

export const sleep = (ms: number) =>
  new Promise((resume) => setTimeout(resume, ms));

export const SESSION = "s-sikemux";

export function quantile(values: number[], at: number): number {
  const sorted = [...values].sort((a, b) => a - b);
  return Math.round(
    sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * at))] ?? 0,
  );
}

export function spread(values: number[]) {
  return {
    p50Ms: quantile(values, 0.5),
    p90Ms: quantile(values, 0.9),
    p99Ms: quantile(values, 0.99),
    longestMs: quantile(values, 1),
  };
}

/** Every frame's length while `during` runs and for `ms` after it. */
export async function frames(
  ms: number,
  during?: () => void,
): Promise<number[]> {
  const lengths: number[] = [];
  let last = performance.now();
  let watching = true;
  const tick = () => {
    const now = performance.now();
    lengths.push(now - last);
    last = now;
    if (watching) requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
  await sleep(30);
  lengths.length = 0;
  during?.();
  await sleep(ms);
  watching = false;
  return lengths;
}

/** A headless browser may draw at 60Hz, so a frame is the cadence measured at rest. */
export async function restingFrame(): Promise<number> {
  const idle = await frames(1000);
  return [...idle].sort((a, b) => a - b)[idle.length >> 1] ?? 1000 / 120;
}

/** How far the frames ran past the display's frame in all, and the longest of them. */
export function jank(lengths: number[], frameMs: number) {
  return {
    overBudgetMs: Math.round(
      lengths.reduce((sum, length) => sum + Math.max(0, length - frameMs), 0),
    ),
    longestMs: Math.round(Math.max(0, ...lengths)),
  };
}

/** Every frame's length from `during` until `done` holds and for `ms` after, and how long `done` took to hold. */
export async function framesUntil(
  done: () => boolean,
  during: () => void,
  ms = 500,
): Promise<{ lengths: number[]; doneMs: number }> {
  const lengths: number[] = [];
  const began = performance.now();
  let last = began;
  let doneAt = 0;
  await new Promise<void>((settle, fail) => {
    const tick = () => {
      const now = performance.now();
      lengths.push(now - last);
      last = now;
      if (!doneAt && done()) doneAt = now;
      if (doneAt && now - doneAt >= ms) settle();
      else if (now - began > 120_000) fail(new Error("it never settled"));
      else requestAnimationFrame(tick);
    };
    during();
    requestAnimationFrame(tick);
  });
  return { lengths, doneMs: Math.round(doneAt - began) };
}

/** Runs `step` once a frame until it answers false, and returns the frame lengths once the first two have settled. */
export async function everyFrameWhile(step: () => boolean): Promise<number[]> {
  const lengths: number[] = [];
  let last = performance.now();
  await new Promise<void>((done) => {
    const tick = () => {
      const now = performance.now();
      lengths.push(now - last);
      last = now;
      if (step()) requestAnimationFrame(tick);
      else done();
    };
    requestAnimationFrame(tick);
  });
  return lengths.slice(2);
}

/** Runs `step` once a frame, `count` times, and returns the frame lengths once the first two have settled. */
export function everyFrame(count: number, step: () => void): Promise<number[]> {
  let taken = 0;
  return everyFrameWhile(() => {
    step();
    return ++taken < count;
  });
}
