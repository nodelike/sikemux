import type { MarkdownOptions, MdElement } from "@shared/markdown/types";

/*
 * The core parses markdown in Rust; the page runs the same parser, built to WebAssembly, so a
 * shared chat reads here exactly as it does in the app.
 */
interface Request {
  readonly text: string;
  readonly options: MarkdownOptions;
  readonly skip: number;
}

interface Parser {
  memory: WebAssembly.Memory;
  take(len: number): number;
  give(at: number, len: number): void;
  parse(at: number, len: number): bigint;
}

let parser: Promise<Parser> | null = null;

function load(): Promise<Parser> {
  parser ??= WebAssembly.instantiateStreaming(fetch("/markdown.wasm")).then(
    ({ instance }) => instance.exports as unknown as Parser,
  );
  return parser;
}

export const markdownApi = {
  async parse(request: Request): Promise<MdElement[]> {
    const { memory, take, give, parse } = await load();
    const input = new TextEncoder().encode(JSON.stringify(request));
    const at = take(input.length);
    new Uint8Array(memory.buffer, at, input.length).set(input);
    const answer = parse(at, input.length);
    give(at, input.length);
    const place = Number(answer >> 32n);
    const length = Number(answer & 0xffffffffn);
    const text = new TextDecoder().decode(
      new Uint8Array(memory.buffer, place, length),
    );
    give(place, length);
    return JSON.parse(text) as MdElement[];
  },
};
