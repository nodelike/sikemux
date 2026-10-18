import { spread } from "./measure";

/*
 * How long the app takes from the start of the page to drawing its first
 * pane, and the longest frames on the way there and in the two seconds after,
 * as recorded by the script at the top of perf.html.
 */

interface Boot {
  firstPane: number;
  frames: { at: number; length: number }[];
}

export async function startup() {
  const { firstPane, frames } = await (
    window as unknown as { perfBoot: Promise<Boot> }
  ).perfBoot;
  const lengths = (drawn: boolean) =>
    frames
      .filter(({ at }) => at > firstPane === drawn)
      .map(({ length }) => length);
  return {
    firstPaneMs: Math.round(firstPane),
    beforePane: spread(lengths(false)),
    afterPane: spread(lengths(true)),
  };
}
