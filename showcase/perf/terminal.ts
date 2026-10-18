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
 * Floods a terminal with a colourful build log as fast as it takes it, the
 * way `cat` of a big log does, while keys are typed into it, then scrolls
 * back through what it kept.
 */

const WINDOW = "w-sikemux-term";
const PANE = "t-dev";

const paint = (code: number, text: string) => `\x1b[${code}m${text}\x1b[0m`;

function buildLog(bytes: number): Uint8Array {
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
  return new TextEncoder().encode(`${lines.join("\r\n")}\r\n`);
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
    `[data-pane-id="${PANE}"] .xterm-helper-textarea`,
  );
  while ((!output || !input) && performance.now() < deadline) {
    await sleep(250);
    output = backend.terminalOutput(PANE);
    input = document.querySelector<HTMLTextAreaElement>(
      `[data-pane-id="${PANE}"] .xterm-helper-textarea`,
    );
  }
  if (!output || !input) throw new Error("the terminal is not on screen");
  input.focus();
  const frameMs = await restingFrame();

  const pressedAt: number[] = [];
  const typed: number[] = [];
  const onKey = (event: KeyboardEvent) => pressedAt.push(event.timeStamp);
  // On the window, since xterm's own listener on the terminal sends the key before any other there runs.
  window.addEventListener("keydown", onKey, { capture: true });
  // The shell echoes what is typed into the output, ahead of what the flood has not yet written.
  backend.on("pty_write", ({ data }) => {
    for (const key of String(data)) {
      const at = pressedAt.shift();
      if (at === undefined) continue;
      void output
        .write(new TextEncoder().encode(key), { ahead: true })
        .then((end) => output.acked(end))
        .then(() =>
          requestAnimationFrame(() => typed.push(performance.now() - at)),
        );
    }
  });

  const log = buildLog(1 << 20);
  const start = output.sent;
  const until = performance.now() + seconds * 1000;
  const flood = async () => {
    while (performance.now() < until) await output.write(log);
  };
  let pressed = 0;
  const type = async () => {
    for (; pressed < keys && press; pressed++) {
      await sleep((seconds * 1000) / (keys + 1));
      await press(String.fromCharCode(97 + (pressed % 26)));
    }
  };
  const flooding = await frames(seconds * 1000, () => {
    void flood();
    void type();
  });
  await output.acked(output.sent);
  await sleep(500);
  window.removeEventListener("keydown", onKey, { capture: true });
  const megabytes = (output.sent - start) / (1 << 20);
  // A key whose echo never came back counts as one that took forever.
  const unechoed = Array<number>(pressed - typed.length).fill(Infinity);

  const screen = document.querySelector<HTMLElement>(
    `[data-pane-id="${PANE}"] .xterm-screen`,
  );
  if (!screen) throw new Error("the terminal screen is not on screen");
  const wheel = (deltaY: number) => () =>
    screen.dispatchEvent(
      new WheelEvent("wheel", { deltaY, bubbles: true, cancelable: true }),
    );
  const sliderTop = () =>
    parseFloat(
      document.querySelector<HTMLElement>(
        `[data-pane-id="${PANE}"] .scrollbar.vertical .slider`,
      )?.style.top ?? "",
    );
  const bottom = sliderTop();
  const scroll = spread(await everyFrame(240, wheel(-40)));
  if (!(sliderTop() < bottom)) throw new Error("the scrollback did not scroll");
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
