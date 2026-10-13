export type Align = 'left' | 'center' | 'right' | null;

export type Block =
  | { kind: 'paragraph'; text: string }
  | { kind: 'heading'; level: number; text: string }
  | { kind: 'item'; depth: number; marker: string; checked: boolean | null; text: string }
  | { kind: 'code'; lang: string; text: string }
  | { kind: 'quote'; text: string }
  | { kind: 'rule' }
  | { kind: 'table'; align: Align[]; head: string[]; rows: string[][] };

export type Inline =
  | string
  | { kind: 'code'; text: string }
  | { kind: 'bold' | 'italic' | 'strike'; children: Inline[] }
  | { kind: 'link'; url: string; children: Inline[] };

const FENCE = /^\s*(```+|~~~+)\s*(\S*)/;
const HEADING = /^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$/;
const RULE = /^\s{0,3}([-*_])(\s*\1){2,}\s*$/;
const ITEM = /^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$/;
const TASK = /^\[([ xX])\]\s+(.*)$/;
const QUOTE = /^\s{0,3}>\s?(.*)$/;
const DELIMITER = /^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$/;

/** A row's cells, split on the pipes that are not escaped or inside backticks. */
export function cells(line: string): string[] {
  const out: string[] = [];
  let cell = '';
  let ticks = 0;
  const text = line.trim();
  for (let index = 0; index < text.length; index += 1) {
    const char = text[index];
    if (char === '\\' && text[index + 1] === '|') {
      cell += '|';
      index += 1;
    } else if (char === '`') {
      ticks ^= 1;
      cell += char;
    } else if (char === '|' && !ticks) {
      out.push(cell.trim());
      cell = '';
    } else cell += char;
  }
  out.push(cell.trim());
  if (text.startsWith('|')) out.shift();
  if (text.endsWith('|') && out.length > 0) out.pop();
  return out;
}

function alignOf(cell: string): Align {
  const left = cell.startsWith(':');
  const right = cell.endsWith(':');
  return left && right ? 'center' : right ? 'right' : left ? 'left' : null;
}

const isTableStart = (lines: string[], index: number) =>
  lines[index].includes('|') &&
  index + 1 < lines.length &&
  lines[index + 1].includes('-') &&
  DELIMITER.test(lines[index + 1]) &&
  lines[index + 1].includes('|');

export function blocks(source: string): Block[] {
  const out: Block[] = [];
  const lines = source.split('\n');
  let paragraph: string[] = [];
  const flush = () => {
    if (paragraph.length) out.push({ kind: 'paragraph', text: paragraph.join(' ') });
    paragraph = [];
  };
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    const fence = line.match(FENCE);
    if (fence) {
      flush();
      const close = fence[1];
      const code: string[] = [];
      for (index += 1; index < lines.length && !lines[index].trimStart().startsWith(close); index += 1) code.push(lines[index]);
      out.push({ kind: 'code', lang: fence[2], text: code.join('\n') });
      continue;
    }
    if (!line.trim()) {
      flush();
      continue;
    }
    if (isTableStart(lines, index)) {
      flush();
      const head = cells(line);
      const align = cells(lines[index + 1]).map(alignOf);
      const width = Math.max(head.length, align.length);
      const rows: string[][] = [];
      for (index += 2; index < lines.length && lines[index].includes('|') && lines[index].trim(); index += 1)
        rows.push(cells(lines[index]));
      index -= 1;
      const fit = (row: string[]) => Array.from({ length: width }, (_, column) => row[column] ?? '');
      out.push({
        kind: 'table',
        align: Array.from({ length: width }, (_, column) => align[column] ?? null),
        head: fit(head),
        rows: rows.map(fit),
      });
      continue;
    }
    const quote = line.match(QUOTE);
    if (quote) {
      flush();
      const quoted = [quote[1]];
      for (let next = lines[index + 1]?.match(QUOTE); next; next = lines[index + 1]?.match(QUOTE)) {
        quoted.push(next[1]);
        index += 1;
      }
      out.push({ kind: 'quote', text: quoted.join('\n') });
      continue;
    }
    const heading = line.match(HEADING);
    if (heading) {
      flush();
      out.push({ kind: 'heading', level: heading[1].length, text: heading[2] });
      continue;
    }
    if (RULE.test(line)) {
      flush();
      out.push({ kind: 'rule' });
      continue;
    }
    const item = line.match(ITEM);
    if (item) {
      flush();
      const task = item[3].match(TASK);
      const ordered = /\d/.test(item[2][0]);
      out.push({
        kind: 'item',
        depth: Math.min(Math.floor(item[1].replace(/\t/g, '    ').length / 2), 4),
        marker: ordered ? `${item[2].slice(0, -1)}.` : '•',
        checked: task ? task[1] !== ' ' : null,
        text: task ? task[2] : item[3],
      });
      continue;
    }
    const last = out[out.length - 1];
    if (!paragraph.length && last?.kind === 'item' && /^\s/.test(line)) {
      out[out.length - 1] = { ...last, text: `${last.text} ${line.trim()}` };
      continue;
    }
    paragraph.push(line.trim());
  }
  flush();
  return out;
}

/**
 * How much of a message can no longer change as it streams in: everything up to its last
 * blank line or closed code fence. Blocks before that point parse the same whatever follows.
 */
export function settledLength(source: string): number {
  let settled = 0;
  let fenced = false;
  for (let start = 0, end = source.indexOf('\n'); end !== -1; start = end + 1, end = source.indexOf('\n', start)) {
    const line = source.slice(start, end);
    if (FENCE.test(line)) {
      fenced = !fenced;
      if (!fenced) settled = end + 1;
    } else if (!fenced && !line.trim()) {
      settled = end + 1;
    }
  }
  return settled;
}

const PUNCTUATION = /[!-/:-@[-`{-~]/;
const WORD = /[A-Za-z0-9\u00c0-\uffff]/;
const BARE_URL = /^https?:\/\/[^\s<>()]+/;

/** The index of `mark` closing a run that opened at `from`, or -1. A run never closes on a space. */
function closing(text: string, mark: string, from: number): number {
  for (let at = text.indexOf(mark, from); at !== -1; at = text.indexOf(mark, at + 1)) {
    if (at > from && text[at - 1] !== ' ' && (mark !== '_' || !WORD.test(text[at + 1] ?? ''))) return at;
  }
  return -1;
}

/** Where the `]` matching the `[` at `open` is, counting brackets in between. */
function bracket(text: string, open: number): number {
  let depth = 0;
  for (let at = open; at < text.length; at += 1) {
    if (text[at] === '\\') at += 1;
    else if (text[at] === '[') depth += 1;
    else if (text[at] === ']' && --depth === 0) return at;
  }
  return -1;
}

export function inlines(text: string): Inline[] {
  const out: Inline[] = [];
  let plain = '';
  const push = (node: Inline) => {
    if (plain) out.push(plain);
    plain = '';
    out.push(node);
  };
  for (let at = 0; at < text.length;) {
    const char = text[at];
    if (char === '\\' && PUNCTUATION.test(text[at + 1] ?? '')) {
      plain += text[at + 1];
      at += 2;
      continue;
    }
    if (char === '`') {
      let ticks = 1;
      while (text[at + ticks] === '`') ticks += 1;
      const fence = '`'.repeat(ticks);
      const end = text.indexOf(fence, at + ticks);
      if (end !== -1) {
        push({ kind: 'code', text: text.slice(at + ticks, end).replace(/^ (.*) $/, '$1') });
        at = end + ticks;
        continue;
      }
      plain += fence;
      at += ticks;
      continue;
    }
    if (char === '[' || (char === '!' && text[at + 1] === '[')) {
      const open = char === '!' ? at + 1 : at;
      const close = bracket(text, open);
      if (close !== -1 && text[close + 1] === '(') {
        const end = text.indexOf(')', close + 2);
        const url =
          end === -1
            ? ''
            : text
                .slice(close + 2, end)
                .trim()
                .split(/\s+/)[0]
                .replace(/^<|>$/g, '');
        if (end !== -1) {
          const label = text.slice(open + 1, close);
          push({ kind: 'link', url, children: char === '!' ? [label || 'image'] : inlines(label) });
          at = end + 1;
          continue;
        }
      }
    }
    if (char === '<' && /^<https?:\/\//.test(text.slice(at, at + 9))) {
      const end = text.indexOf('>', at);
      if (end !== -1) {
        const url = text.slice(at + 1, end);
        push({ kind: 'link', url, children: [url] });
        at = end + 1;
        continue;
      }
    }
    if (char === 'h' && !WORD.test(text[at - 1] ?? '')) {
      const url = BARE_URL.exec(text.slice(at, at + 2048))?.[0].replace(/[.,;:!?'"]+$/, '');
      if (url) {
        push({ kind: 'link', url, children: [url] });
        at += url.length;
        continue;
      }
    }
    const pair = text.slice(at, at + 2);
    if ((pair === '**' || pair === '__' || pair === '~~') && text[at + 2] && text[at + 2] !== ' ') {
      const end = closing(text, pair, at + 2);
      if (end !== -1) {
        push({ kind: pair === '~~' ? 'strike' : 'bold', children: inlines(text.slice(at + 2, end)) });
        at = end + 2;
        continue;
      }
    }
    if (
      (char === '*' || (char === '_' && !WORD.test(text[at - 1] ?? ''))) &&
      text[at + 1] &&
      text[at + 1] !== ' ' &&
      text[at + 1] !== char
    ) {
      const end = closing(text, char, at + 1);
      if (end !== -1) {
        push({ kind: 'italic', children: inlines(text.slice(at + 1, end)) });
        at = end + 1;
        continue;
      }
    }
    plain += char;
    at += 1;
  }
  if (plain) out.push(plain);
  return out;
}
