import type {
  ChatChange,
  ChatHead,
  ChatMessage,
  ChatView,
} from "@shared/wire.generated";
import { applyChanges, emptyChat } from "@shared/chat/rows";
import type { ShowcaseBackend } from "../backend";
import { AGENTS } from "../world/workspace";
import {
  SESSION,
  everyFrameWhile,
  framesUntil,
  restingFrame,
  showcase,
  sleep,
  spread,
} from "./measure";

/*
 * Times an agent's chat the way the GPUI transcript's example does, so the
 * two compare: opening 2,000 rows, scrolling them to the top, scrolling up
 * from the last 200 while earlier pages arrive, and a 4,000-token answer
 * streaming in at 500 tokens a second. The rows and every change to them are
 * the core's own, made by its fold from the same words the example uses.
 */

const AGENT = AGENTS.replay.id;
const FEED = "perf-chat";
const SCROLL_STEP = 60;
const CHARS_PER_FLUSH = 32;
const FLUSH_MS = 16;
const PAGE_DELAY_MS = 30;
const STREAM_TAIL = 40;

const WORDS = [
  "the",
  "core",
  "folds",
  "each",
  "event",
  "into",
  "rows",
  "and",
  "sends",
  "only",
  "what",
  "changed",
  "since",
  "last",
  "frame",
  "so",
  "a",
  "long",
  "chat",
  "stays",
  "cheap",
  "to",
  "draw",
  "here",
];

/** Markdown and how many characters of it a reader sees, as the example counts them. */
interface Written {
  md: string;
  seen: number;
}

/** The example's generator, written out as markdown: the same words every run, so runs compare. */
class Words {
  private state: bigint;

  constructor(seed: number) {
    this.state = BigInt(seed);
  }

  private next(below: number): number {
    this.state =
      (this.state * 6364136223846793005n + 1442695040888963407n) &
      0xffff_ffff_ffff_ffffn;
    return Number(this.state >> 33n) % Math.max(1, below);
  }

  private sentence(): string {
    const count = 6 + this.next(14);
    const words = Array.from(
      { length: count },
      () => WORDS[this.next(WORDS.length)],
    );
    const sentence = words.join(" ");
    return `${sentence[0].toUpperCase()}${sentence.slice(1)}. `;
  }

  private inline(): Written {
    const first = this.sentence();
    const [md, seen] = [
      () => ["`src/chat/rows.rs`", "src/chat/rows.rs"],
      () => {
        const strong = this.sentence().trimEnd();
        return [`**${strong}**`, strong];
      },
      () => ["[the docs](https://sikemux.com)", "the docs"],
      () => ["*once per frame*", "once per frame"],
    ][this.next(4)]();
    const last = ` ${this.sentence()}`;
    return {
      md: `${first}${md}${last}`.trimEnd(),
      seen: first.length + seen.length + last.length,
    };
  }

  private items(count: number, marker: (index: number) => string): Written {
    const items = Array.from({ length: count }, () => this.inline());
    return {
      md: items.map((item, index) => `${marker(index)} ${item.md}`).join("\n"),
      seen: items.reduce((sum, item) => sum + item.seen, 0),
    };
  }

  private block(): Written {
    const kind = this.next(10);
    if (kind === 0) {
      const level = 2 + this.next(2);
      const text = this.sentence().slice(0, -2);
      return { md: `${"#".repeat(level)} ${text}`, seen: text.length };
    }
    if (kind === 1 || kind === 2)
      return this.items(2 + this.next(4), () => "-");
    if (kind === 3)
      return this.items(2 + this.next(3), (index) => `${index + 1}.`);
    if (kind === 4) {
      const lines = Array.from(
        { length: 4 + this.next(20) },
        (_, index) => `    let row_${index} = rows.put(message(${index}));\n`,
      ).join("");
      const code = `fn main() {\n${lines}}\n`;
      return { md: `\`\`\`rust\n${code}\`\`\``, seen: code.length };
    }
    if (kind === 5) {
      const rows = Array.from({ length: 3 + this.next(4) }, (_, index) => [
        `step ${index}`,
        String(100 + this.next(900)),
        this.sentence().trimEnd(),
      ]);
      const line = (cells: string[]) => `| ${cells.join(" | ")} |`;
      return {
        md: [
          line(["Phase", "Frames", "Note"]),
          line(["---", "---:", "---"]),
          ...rows.map(line),
        ].join("\n"),
        seen: rows.flat().join("").length,
      };
    }
    const text = this.inline();
    return kind === 6 ? { ...text, md: `> ${text.md}` } : text;
  }

  private blocks(count: number): string {
    return Array.from({ length: count }, () => this.block().md).join("\n\n");
  }

  /** A short prompt and an answer of one to five blocks, by turns. */
  rows(count: number): string[] {
    return Array.from({ length: count }, (_, index) =>
      index % 2 === 0
        ? this.sentence().trimEnd()
        : this.blocks(1 + this.next(5)),
    );
  }

  /** About `tokens` tokens of answer, at four characters a reader sees to a token. */
  answer(tokens: number): string {
    const blocks: string[] = [];
    for (let seen = 0; seen < tokens * 4;) {
      const block = this.block();
      blocks.push(block.md);
      seen += block.seen;
    }
    return blocks.join("\n\n");
  }
}

async function fold(request: object) {
  const response = await fetch("/__showcase/fold", {
    method: "POST",
    body: JSON.stringify({ chat: FEED, ...request }),
  });
  if (!response.ok)
    throw new Error(`the fold failed: ${await response.text()}`);
  return (await response.json()) as {
    changes?: ChatChange[];
    view?: ChatView;
  };
}

const said = (sessionUpdate: string, messageId: string, text: string) => ({
  kind: "session_update",
  payload: {
    sessionId: `session-${FEED}`,
    update: { sessionUpdate, messageId, content: { type: "text", text } },
  },
});

interface Folded {
  head: ChatHead;
  rows: ChatMessage[];
  /** The answer as the core sends it, one list of changes for each flush. */
  flushes: ChatChange[][];
  answerChars: number;
}

/** The core's fold of the chat, and of the answer streaming into it 32 characters a flush. */
async function folded(count: number, tokens: number): Promise<Folded> {
  const { changes = [] } = await fold({
    reset: true,
    events: [
      {
        kind: "ready",
        payload: {
          sessionId: `session-${FEED}`,
          capabilities: {},
          setup: { configOptions: [] },
        },
      },
      ...new Words(7)
        .rows(count)
        .map((text, index) =>
          said(
            index % 2 === 0 ? "user_message_chunk" : "agent_message_chunk",
            `${FEED}-${index}`,
            text,
          ),
        ),
    ],
  });
  const rows = applyChanges(emptyChat, changes).messages;
  if (rows.length !== count)
    throw new Error(`the fold made ${rows.length} rows, not ${count}`);
  const { view } = await fold({ view: true });
  const answer = new Words(11).answer(tokens);
  const flush = async (events: object[]) =>
    (await fold({ events })).changes ?? [];
  const flushes = [
    await flush([
      { kind: "turn_started", payload: { at: Date.now() } },
      said("user_message_chunk", `${FEED}-ask`, "Write it all up."),
    ]),
  ];
  for (let at = 0; at < answer.length; at += CHARS_PER_FLUSH)
    flushes.push(
      await flush([
        said(
          "agent_message_chunk",
          `${FEED}-answer`,
          answer.slice(at, at + CHARS_PER_FLUSH),
        ),
      ]),
    );
  flushes.push(
    await flush([
      {
        kind: "turn_completed",
        payload: { stopReason: "end_turn", at: Date.now() },
      },
    ]),
  );
  return { head: view!.head, rows, flushes, answerChars: answer.length };
}

/**
 * Answers the pane's attach with the view it is opened on, and its trips back
 * with pages of the rows. Every agent's pane attaches as the showcase starts,
 * so opening the chat again is the core telling it to attach again.
 */
function serve(backend: ShowcaseBackend, rows: ChatMessage[], page: number) {
  let shown: ChatView | null = null;
  let seq = 0;
  const served = { pages: 0, all: false };
  const invoke = backend.invoke.bind(backend);
  backend.invoke = (async (command: string, args?: unknown) => {
    const { agentId, request } = (args ?? {}) as {
      agentId?: string;
      request?: { op: string; agentId: string; before: number };
    };
    if (command === "acp_attach" && agentId === AGENT)
      return { status: "live", view: shown, feed: FEED, seq };
    if (request?.op === "acpHistory" && request.agentId === AGENT) {
      await sleep(PAGE_DELAY_MS);
      const start = Math.max(0, request.before - page);
      served.pages += 1;
      served.all = start === 0;
      return {
        page: {
          rows: rows.slice(start, request.before),
          ...(start > 0 ? { olderBefore: start } : {}),
        },
      };
    }
    return invoke(command, args);
  }) as typeof backend.invoke;
  const event = (kind: string, payload: unknown) =>
    backend.emit("acp_event", { agentId: AGENT, kind, seq, payload });
  return {
    served,
    reattach(view: ChatView) {
      shown = view;
      event("reattach", null);
    },
    send(changes: ChatChange[]) {
      seq += 1;
      event("changes", changes);
    },
  };
}

/** How long each frame keeps the page's thread: from its first animation callback to the task after its drawing. */
function renderWork(): () => number[] {
  const lengths: number[] = [];
  const channel = new MessageChannel();
  let began = 0;
  let watching = true;
  channel.port1.onmessage = () => lengths.push(performance.now() - began);
  const tick = () => {
    if (!watching) return;
    began = performance.now();
    channel.port2.postMessage(null);
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
  return () => {
    watching = false;
    channel.port1.close();
    return lengths;
  };
}

interface Usage {
  contentCpuPercent: number;
  contentRssMb: number;
  gpuCpuPercent: number;
  uiCpuPercent: number;
}

/** The WebKit processes' CPU and memory over a phase, which `scripts/perf.mjs` reads from outside the page. */
const usage = (watching: boolean) =>
  (
    window as unknown as {
      perfUsage?: (watching: boolean) => Promise<Usage | null>;
    }
  ).perfUsage?.(watching);

async function waitFor(holds: () => boolean) {
  const deadline = performance.now() + 60_000;
  while (!holds()) {
    if (performance.now() > deadline) throw new Error("the chat never came");
    await sleep(50);
  }
}

/** The row at the top of the view and where it is. */
function rowAtTop(scroller: HTMLElement) {
  const view = scroller.getBoundingClientRect();
  for (const offset of [1, 24, 64]) {
    const row = document
      .elementFromPoint(view.left + view.width / 2, view.top + offset)
      ?.closest(".chat-row");
    if (row) return { row, top: row.getBoundingClientRect().top };
  }
  return null;
}

export async function chat({
  rows: count = 2000,
  page = 200,
  tokens = 4000,
}: { rows?: number; page?: number; tokens?: number } = {}) {
  const { backend, cmd } = showcase();
  const { head, rows, flushes, answerChars } = await folded(count, tokens);
  const chatServer = serve(backend, rows, page);
  const { served } = chatServer;
  // Playwright's WebKit anchors scrolling and the app's does not, which here keeps a streaming answer from following the bottom.
  const unanchored = document.createElement("style");
  unanchored.textContent = ".chat-scroll { overflow-anchor: none; }";
  document.head.append(unanchored);
  cmd.selectSession(SESSION);
  cmd.selectWindowId(`w-${AGENT}`);
  await sleep(1500);

  const frameMs = await restingFrame();
  const summary = (lengths: number[]) => ({
    ...spread(lengths),
    frames: lengths.length,
    dropped: lengths.filter((length) => length > frameMs * 1.5).length,
  });
  const phase = async <T extends object>(run: () => Promise<T>) => {
    await usage(true);
    const stop = renderWork();
    const result = await run();
    const render = spread(stop());
    return { ...result, render, usage: (await usage(false)) ?? null };
  };
  const content = () =>
    document.querySelector<HTMLElement>(
      `[data-pane-id="${AGENT}"] .chat-scroll-content`,
    );
  const firstRow = () =>
    content()?.querySelector<HTMLElement>(":scope > .chat-row") ?? null;
  /** Empties the chat, then opens it on `view` and times the frames until its first row is drawn, as a pane opening it does. */
  const open = async (view: ChatView) => {
    const before = content()?.parentElement;
    if (before) before.scrollTop = 0;
    chatServer.reattach({ head, rows: [] });
    await waitFor(() => !firstRow());
    await sleep(500);
    const shown = await framesUntil(
      () => firstRow()?.dataset.index === "0",
      () => chatServer.reattach(view),
    );
    await sleep(1000);
    return shown;
  };
  const shownChat = () => {
    const held = content()!;
    const scroller = held.parentElement!;
    const upOnce = () => {
      const before = scroller.scrollTop;
      scroller.scrollTop = before - SCROLL_STEP;
      return scroller.scrollTop < before;
    };
    return { held, scroller, upOnce };
  };

  const opened = await phase(async () => {
    const { lengths, doneMs } = await open({ head, rows });
    return { ...summary(lengths), settledMs: doneMs };
  });

  const scroll = await phase(async () =>
    summary(await everyFrameWhile(shownChat().upOnce)),
  );
  if (scroll.frames < 10) throw new Error("the transcript did not scroll up");

  await open({
    head,
    rows: rows.slice(-page),
    olderBefore: rows.length - page,
  });
  const places: number[] = [];
  const paging = await phase(async () => {
    const { held, scroller, upOnce } = shownChat();
    let anchor: ReturnType<typeof rowAtTop> = null;
    let count = held.childElementCount;
    let settling = false;
    let steps = 0;
    const lengths = await everyFrameWhile(() => {
      if (!settling && held.childElementCount !== count) {
        // Holds still a frame, so whatever puts the reader's place back has drawn.
        settling = true;
        return true;
      }
      if (settling && anchor)
        places.push(
          Math.round(anchor.row.getBoundingClientRect().top - anchor.top),
        );
      settling = false;
      count = held.childElementCount;
      const moved = upOnce();
      anchor = rowAtTop(scroller);
      const all = served.all && count >= rows.length;
      return (moved || !all) && ++steps < 60_000;
    });
    return {
      ...summary(lengths),
      pages: served.pages,
      placeKept: places.filter((moved) => moved === 0).length,
      jumpsPx: places.filter((moved) => moved !== 0),
    };
  });

  await open({ head, rows: rows.slice(-STREAM_TAIL) });
  const stream = await phase(async () => {
    const { scroller } = shownChat();
    let sent = 0;
    const began = performance.now();
    const flushing = window.setInterval(() => {
      const due = Math.min(
        flushes.length,
        Math.floor((performance.now() - began) / FLUSH_MS) + 1,
      );
      while (sent < due) chatServer.send(flushes[sent++]);
    }, 4);
    const { lengths } = await framesUntil(
      () => sent === flushes.length,
      () => {},
    );
    window.clearInterval(flushing);
    return {
      ...summary(lengths),
      flushes: flushes.length,
      chars: answerChars,
      fromBottomPx: Math.round(-scroller.scrollTop),
    };
  });

  return { frameMs, rows: count, open: opened, scroll, paging, stream };
}
