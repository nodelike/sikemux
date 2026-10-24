import { SIKEMUX_THEMES, testPalette } from "../../src/test/themes";
import { wearPicked } from "../../src/themes/bus";
import { SESSION, showcase, sleep, spread } from "./measure";

/*
 * Steps through themes the way the theme picker previews them on each arrow
 * key, and times each from the call to the frame after it is drawn.
 */

const WINDOWS = ["w-sikemux-term", "w-sikemux-files", "w-sikemux-git"];

const frame = () =>
  new Promise<void>((done) => requestAnimationFrame(() => done()));

export async function theme({ steps = 12 }: { steps?: number } = {}) {
  const { cmd } = showcase();
  cmd.selectSession(SESSION);
  const result: Record<string, ReturnType<typeof spread>> = {};
  for (const id of WINDOWS) {
    cmd.selectWindowId(id);
    await sleep(2000);
    const shown: number[] = [];
    for (let step = 0; step < steps; step++) {
      const start = performance.now();
      wearPicked(
        testPalette(SIKEMUX_THEMES[(step * 7 + 3) % SIKEMUX_THEMES.length]),
      );
      await frame();
      await frame();
      shown.push(performance.now() - start);
      await sleep(100);
    }
    result[id] = spread(shown);
  }
  return result;
}
