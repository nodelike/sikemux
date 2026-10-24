import { parseLines } from "../terminalFrames";
import {
  SESSION,
  everyFrame,
  frames,
  restingFrame,
  showcase,
  sleep,
  spread,
} from "./measure";

/*
 * Floods a terminal with a colourful build log the way `cat` of a big log
 * does, at the rate the core reads it, while keys are typed into it, then
 * scrolls back through what it kept. The core sends a frame of the screen
 * each time the last one was drawn, so every frame carries a full screen.
 */

/** What the core reads of the log in one 60Hz frame: about 29 MB/s, as measured. */
const LOG_BYTES_PER_FRAME = 485_000;

const WINDOW = "w-sikemux-term";
const PANE = "t-dev";

const paint = (code: number, text: string) => `\x1b[${code}m${text}\x1b[0m`;

function buildLog(bytes: number): string {
  const crates = ["tokio", "serde_json", "sikemux-core", "vt100", "hyper"];
  const lines: string[] = [];
  let size = 0;
  for (let line = 0; size < bytes; line++) {
    const crate = crates[line % crates.length];
    const text =
      line % 11 === 0
        ? `${paint(33, "warning")}: unused variable \`chunk_${line}\` in ${paint(36, `src/${crate}/lib.rs:${line % 900}:${line % 80}`)}`
        : line % 7 === 0
          ? `  ${paint(32, "✓")} ${crate}::tests::case_${line} ${paint(2, `(${line % 300}ms)`)}`
          : `   ${paint(1, paint(32, "Compiling"))} ${crate} v1.${line % 40}.${line % 9} ${paint(2, `(/home/build/${crate}/crates/${crate}-${line})`)}`;
    lines.push(text);
    size += text.length + 2;
  }
  return `${lines.join("\r\n")}\r\n`;
}

/** Types through Playwright when it drives the page, so keys take WebKit's real input path. */
const press = (
  window as unknown as { perfPress?: (key: string) => Promise<void> }
).perfPress;

export async function terminal({
  seconds = 5,
  keys = 25,
}: { seconds?: number; keys?: number } = {}) {
  const { backend, cmd } = showcase();
  cmd.selectSession(SESSION);
  cmd.selectWindowId(WINDOW);
  await sleep(1500);
  const deadline = performance.now() + 30_000;
  let output = backend.terminalOutput(PANE);
  let input = document.querySelector<HTMLTextAreaElement>(
    `[data-pane-id="${PANE}"] .terminal-input`,
  );
  while ((!output || !input) && performance.now() < deadline) {
    await sleep(250);
    output = backend.terminalOutput(PANE);
    input = document.querySelector<HTMLTextAreaElement>(
      `[data-pane-id="${PANE}"] .terminal-input`,
    );
  }
  if (!output || !input) throw new Error("the terminal is not on screen");
  input.focus();
  const frameMs = await restingFrame();

  const pressedAt: number[] = [];
  const typed: number[] = [];
  const onKey = (event: KeyboardEvent) => pressedAt.push(event.timeStamp);
  window.addEventListener("keydown", onKey, { capture: true });
  // The shell echoes what is typed into the next frame.
  backend.on("pty_input", ({ input: typedInput }) => {
    const { text } = typedInput as { text?: string };
    for (const key of text ?? "") {
      const at = pressedAt.shift();
      if (at === undefined) continue;
      output.write(key);
      void output
        .drawnSoon()
        .then(() =>
          requestAnimationFrame(() => typed.push(performance.now() - at)),
        );
    }
  });

  const log = parseLines(buildLog(1 << 20));
  const linesPerFrame = Math.round(
    (LOG_BYTES_PER_FRAME / (1 << 20)) * log.length,
  );
  let offset = 0;
  let fed = 0;
  const until = performance.now() + seconds * 1000;
  const flood = () => {
    output.beforeFrame = () => {
      if (performance.now() >= until) {
        output.beforeFrame = null;
        return;
      }
      const lines = log.slice(offset, offset + linesPerFrame);
      offset = (offset + linesPerFrame) % log.length;
      output.screen.pushLines(lines);
      fed += LOG_BYTES_PER_FRAME;
    };
    output.changed();
  };
  let pressed = 0;
  const type = async () => {
    for (; pressed < keys && press; pressed++) {
      await sleep((seconds * 1000) / (keys + 1));
      await press(String.fromCharCode(97 + (pressed % 26)));
    }
  };
  const flooding = await frames(seconds * 1000, () => {
    flood();
    void type();
  });
  await sleep(500);
  window.removeEventListener("keydown", onKey, { capture: true });
  const megabytes = fed / (1 << 20);
  // A key whose echo never came back counts as one that took forever.
  const unechoed = Array<number>(pressed - typed.length).fill(Infinity);

  const screen = document.querySelector<HTMLElement>(
    `[data-pane-id="${PANE}"] .terminal-surface`,
  );
  if (!screen) throw new Error("the terminal screen is not on screen");
  const wheel = (deltaY: number) => () =>
    screen.dispatchEvent(
      new WheelEvent("wheel", { deltaY, bubbles: true, cancelable: true }),
    );
  const scrolled = () =>
    document.querySelector<HTMLElement>(
      `[data-pane-id="${PANE}"] .terminal-scrollbar`,
    )?.dataset.scrolled === "true";
  const scroll = spread(await everyFrame(240, wheel(-40)));
  if (!scrolled()) throw new Error("the scrollback did not scroll");
  const fling = spread(await everyFrame(120, wheel(400)));

  return {
    frameMs,
    flood: {
      ...spread(flooding),
      megabytesPerSecond: Math.round((megabytes / seconds) * 10) / 10,
    },
    typing: {
      ...spread([...typed, ...unechoed]),
      keys: pressed,
    },
    scroll,
    fling,
  };
}
