/*
 * The showcase's terminals, sent the way the core sends them: frames of
 * cells in sikemux-wire's grid layout. The showcase's screens are coloured
 * lines, so this reads only colours and line ends, and keeps every line so
 * scrolling back can page through them.
 */

interface Style {
  fg: number;
  bg: number;
  flags: number;
}

interface Run {
  style: Style;
  text: string;
}

const FOREGROUND = 256;
const BACKGROUND = 257;
const PLAIN: Style = { fg: FOREGROUND, bg: BACKGROUND, flags: 0 };
const RGB = 0x1000000;

function sgr(style: Style, params: number[]): Style {
  let next = { ...style };
  for (let index = 0; index < params.length; index++) {
    const code = params[index];
    if (code === 0) next = { ...PLAIN };
    else if (code === 1) next.flags |= 1;
    else if (code === 2) next.flags |= 4;
    else if (code === 3) next.flags |= 2;
    else if (code === 4) next.flags |= 64;
    else if (code === 7) next.flags |= 8;
    else if (code === 9) next.flags |= 32;
    else if (code === 22) next.flags &= ~5;
    else if (code === 23) next.flags &= ~2;
    else if (code === 24) next.flags &= ~64;
    else if (code === 27) next.flags &= ~8;
    else if (code >= 30 && code <= 37) next.fg = code - 30;
    else if (code === 39) next.fg = FOREGROUND;
    else if (code >= 40 && code <= 47) next.bg = code - 40;
    else if (code === 49) next.bg = BACKGROUND;
    else if (code >= 90 && code <= 97) next.fg = code - 90 + 8;
    else if (code >= 100 && code <= 107) next.bg = code - 100 + 8;
    else if (code === 38 || code === 48) {
      const color =
        params[index + 1] === 5
          ? params[index + 2]
          : RGB |
            (params[index + 2] << 16) |
            (params[index + 3] << 8) |
            params[index + 4];
      index += params[index + 1] === 5 ? 2 : 4;
      if (code === 38) next.fg = color;
      else next.bg = color;
    }
  }
  return next;
}

/** A demo terminal's lines, and the frames that show them. */
export type Line = Run[];

/** Lines a demo terminal keeps, as the core keeps 10,000 above its screen. */
const KEPT_LINES = 10_000;

export class DemoScreen {
  private lines: Line[] = [[]];
  /** Lines dropped off the top, which still count toward line numbers. */
  private dropped = 0;
  private style: Style = { ...PLAIN };
  private frameNumber = 0;
  private sentRows = -1;
  cols = 120;
  rows = 40;

  /** Adds lines read once with `parseLines`, which costs nothing per frame. */
  pushLines(lines: readonly Line[]): void {
    this.lines.push(...lines);
    this.trim();
  }

  private trim(): void {
    const extra = this.lines.length - (KEPT_LINES + this.rows);
    if (extra <= 0) return;
    this.lines.splice(0, extra);
    this.dropped += extra;
  }

  write(text: string): void {
    const pattern =
      /\x1b\[([0-9;]*)m|\r?\n|\r|\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|[^\x1b\r\n]+/g;
    for (const match of text.matchAll(pattern)) {
      const token = match[0];
      if (match[1] !== undefined)
        this.style = sgr(
          this.style,
          match[1] === "" ? [0] : match[1].split(";").map(Number),
        );
      else if (token === "\n" || token === "\r\n") this.lines.push([]);
      else if (token === "\r" || token.startsWith("\x1b")) continue;
      else this.lines.at(-1)!.push({ style: this.style, text: token });
    }
    this.trim();
  }

  private get historyEnd(): number {
    return this.dropped + Math.max(0, this.lines.length - this.rows);
  }

  private get historyLen(): number {
    return Math.max(0, this.lines.length - this.rows);
  }

  /** The whole screen as one frame. */
  frame(): Uint8Array {
    const writer = new Writer();
    const top = this.historyEnd;
    const first = top - this.dropped;
    for (let row = 0; row < this.rows; row++) {
      const line = this.lines[first + row];
      if (!line || line.length === 0) {
        writer.op(2);
        writer.varint(row);
        writer.varint(1);
        continue;
      }
      writer.op(3);
      writer.varint(row);
      writer.line(line, this.cols);
    }
    this.frameNumber++;
    const reset = this.sentRows !== this.rows;
    this.sentRows = this.rows;
    const cursorRow = Math.min(this.rows - 1, this.lines.length - 1 - first);
    const cursorCol = Math.min(
      this.cols - 1,
      this.lines
        .at(-1)!
        .reduce((width, run) => width + [...run.text].length, 0),
    );
    return writer.finishFrame([
      this.frameNumber,
      reset ? 1 : 0,
      this.cols,
      this.rows,
      0,
      top,
      this.historyLen,
      cursorRow,
      cursorCol,
    ]);
  }

  /** History lines from `start`, as many as `count`. */
  history(start: number, count: number): Uint8Array {
    const writer = new Writer();
    const first = Math.max(this.dropped, start);
    const end = Math.min(this.historyEnd, first + count);
    for (let line = first; line < end; line++)
      writer.line(this.lines[line - this.dropped], this.cols);
    return writer.finishLines(first, Math.max(0, end - first));
  }
}

class Writer {
  private body: number[] = [];
  private styles: number[] = [];
  private styleIndex = new Map<string, number>();
  private readonly encoder = new TextEncoder();

  varint(value: number, into: number[] = this.body): void {
    let rest = value;
    while (rest >= 0x80) {
      into.push((rest % 0x80) | 0x80);
      rest = Math.floor(rest / 0x80);
    }
    into.push(rest);
  }

  op(code: number): void {
    this.body.push(code);
  }

  private color(color: number): void {
    if (color >= RGB) {
      this.styles.push(
        2,
        (color >> 16) & 0xff,
        (color >> 8) & 0xff,
        color & 0xff,
      );
    } else if (color >= 256) {
      this.styles.push(0);
      this.varint(color, this.styles);
    } else this.styles.push(1, color);
  }

  private style(style: Style): number {
    const key = `${style.fg}:${style.bg}:${style.flags}`;
    let index = this.styleIndex.get(key);
    if (index === undefined) {
      index = this.styleIndex.size;
      this.styleIndex.set(key, index);
      this.varint(style.flags, this.styles);
      this.color(style.fg);
      this.color(style.bg);
    }
    return index;
  }

  line(runs: Run[], cols: number): void {
    this.body.push(0);
    let width = 0;
    for (const run of runs) {
      const characters = [...run.text].slice(0, cols - width);
      if (characters.length === 0) break;
      width += characters.length;
      const bytes = this.encoder.encode(characters.join(""));
      this.varint(this.style(run.style) + 1);
      this.body.push(0);
      this.varint(bytes.length);
      for (const byte of bytes) this.body.push(byte);
    }
    this.body.push(0);
  }

  private tables(out: number[]): void {
    this.varint(this.styleIndex.size, out);
    out.push(...this.styles);
    this.varint(0, out);
  }

  finishFrame(header: number[]): Uint8Array {
    const out: number[] = [];
    const [
      frame,
      flags,
      cols,
      rows,
      epoch,
      historyEnd,
      historyLen,
      cursorRow,
      cursorCol,
    ] = header;
    for (const value of [frame]) this.varint(value, out);
    out.push(flags);
    for (const value of [
      cols,
      rows,
      epoch,
      historyEnd,
      historyLen,
      cursorRow,
      cursorCol,
    ])
      this.varint(value, out);
    out.push(0x81);
    this.varint(0, out);
    this.tables(out);
    out.push(...this.body, 0);
    return Uint8Array.from(out);
  }

  finishLines(start: number, count: number): Uint8Array {
    const out: number[] = [];
    this.varint(0, out);
    this.varint(start, out);
    this.varint(count, out);
    this.tables(out);
    out.push(...this.body);
    return Uint8Array.from(out);
  }
}

/** The lines of coloured text, read once. */
export function parseLines(text: string): Line[] {
  const screen = new DemoScreen();
  screen.write(text);
  return (screen as unknown as { lines: Line[] }).lines;
}
