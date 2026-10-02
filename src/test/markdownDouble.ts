import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import type { MarkdownRequest } from "../api/markdown";
import type { MarkdownOptions, MdElement } from "../markdown/types";

/* Markdown is read in Rust, which a frontend test cannot call. The Rust tests
   read every text in markdownFixtures.json and fail when the blocks stored
   beside it are not what the parser gives, so a component test draws what the
   app would.

   To add a text: run the test with MARKDOWN_FIXTURES=record (and
   --no-file-parallelism) to append it, then fill in its blocks with
   SIKEMUX_MARKDOWN_FIXTURES=update cargo test -p sikemux-markdown. */

interface Fixture {
    text: string;
    options: MarkdownOptions;
    blocks: MdElement[] | null;
}

const FILE = resolve(process.cwd(), "src/test/markdownFixtures.json");

function load(): Fixture[] {
    return (JSON.parse(readFileSync(FILE, "utf8")) as { cases: Fixture[] }).cases;
}

let fixtures: Fixture[] | null = null;

function sameOptions(a: MarkdownOptions, b: MarkdownOptions): boolean {
    return a.gfm === b.gfm && a.htmlAsText === b.htmlAsText && a.fileLinks === b.fileLinks && !!a.htmlImages === !!b.htmlImages;
}

/* A text with no fixture, such as a message caught halfway through arriving,
   reads as plain paragraphs. */
function paragraphs(text: string): MdElement[] {
    return text
        .split(/\n\s*\n/)
        .map((part) => part.trim())
        .filter(Boolean)
        .map((part) => ({ t: "p", c: [part] }));
}

export function parseWithFixtures(request: MarkdownRequest): MdElement[] {
    fixtures ??= load();
    const found = fixtures.find((fixture) => fixture.text === request.text && sameOptions(fixture.options, request.options));
    if (found?.blocks) return found.blocks.slice(request.skip);
    if (!found && process.env.MARKDOWN_FIXTURES === "record") {
        const all = load();
        all.push({ text: request.text, options: request.options, blocks: null });
        writeFileSync(FILE, `${JSON.stringify({ cases: all }, null, 2)}\n`);
        fixtures = all;
    }
    return paragraphs(request.text).slice(request.skip);
}
