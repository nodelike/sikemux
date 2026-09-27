import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const chat = readFileSync(join(process.cwd(), "src", "styles", "chat.css"), "utf8");

function block(selector: string): string {
    const match = chat.match(new RegExp(`(^|\\n)${selector.replace(/[.\\-]/g, "\\$&")}\\s*\\{([^}]*)\\}`));
    expect(match, `${selector} is missing from chat.css`).not.toBeNull();
    return match?.[2] ?? "";
}

const agents = readFileSync(join(process.cwd(), "src", "styles", "agents.css"), "utf8");

describe("the yolo ring", () => {
    /* A gradient that moves by its own background position repaints the ring
       on every frame, and the ring is on screen for as long as the mode is. A
       strip that slides behind a fixed window is the compositor's work. */
    it("moves a strip rather than repainting the border", () => {
        for (const sheet of [chat, agents]) {
            expect(sheet).not.toMatch(/animation:\s*yolo-flow/);
            expect(sheet).toMatch(/\.yolo-ring::before\s*\{[^}]*animation:\s*yolo-slide/);
            expect(sheet).toMatch(/\.yolo-ring\s*\{[^}]*overflow:\s*hidden/);
        }
        expect(agents).toMatch(/@keyframes yolo-slide\s*\{[^@]*transform:\s*translateX\(-50%\)/);
    });
});

/* The glyph is drawn from a font whose icons are far taller than the letters
   beside them. Sat on the shared baseline it rode above the words; given a
   height it pushed the line apart and dropped the full stop after it. */
describe("the icon on a file reference", () => {
    it("is centred on the letters rather than stood on the baseline", () => {
        expect(block(".chat-file-ref .file-glyph")).toMatch(/vertical-align:\s*middle/);
    });

    it("adds no height of its own to the line it lands in", () => {
        expect(block(".chat-file-ref .file-glyph")).toMatch(/line-height:\s*0/);
    });
});

describe("chat overflow", () => {
    /* A user bubble is sized to its own content, and a box sized that way grows
       to fit the longest word in it. A pasted URL is one word, so the bubble
       reached past the pane and took the whole screen sideways with it.
       `break-word` would not have helped: it wraps the text but leaves the box's
       smallest width — and so the bubble — as wide as the URL. */
    it("breaks inside a word that has nowhere else to break", () => {
        expect(block(".chat-markdown")).toMatch(/overflow-wrap:\s*anywhere/);
        expect(block(".chat-message.user .chat-markdown")).toMatch(/width:\s*max-content/);
    });

    /* Agents name a background task with the command they ran, which is a line
       of shell. A name that cannot shrink pushed the row, its stop button and
       the pane's right edge off screen. */
    /* The pane is a grid, and a grid with rows but no columns widens to the
       longest unbreakable run inside it — one queued message holding a link
       pushed the transcript and the composer off the right edge. */
    it("holds the pane to one column of its own width", () => {
        expect(block(".agent-chat-pane")).toMatch(/grid-template-columns:\s*minmax\(0, 1fr\)/);
    });

    /* The chip is sized to its own content inside a user bubble that is sized
       to its content in turn, so a path that refuses to break would have
       widened both past the pane. */
    it("lets a long file name break like the words around it", () => {
        expect(block(".chat-file-ref")).not.toMatch(/white-space:\s*nowrap/);
        expect(chat).not.toMatch(/\.chat-file-ref-name\s*\{[^}]*white-space:\s*nowrap/);
    });

    it("lets a long task name give way rather than the row", () => {
        const name = block(".chat-task-name");
        expect(name).not.toMatch(/flex:\s*none/);
        expect(name).toMatch(/text-overflow:\s*ellipsis/);
        expect(name).toMatch(/min-width:\s*0/);
    });
    it("lets a finished task's command give way rather than its state", () => {
        const name = block(".chat-notice-name");
        expect(name).not.toMatch(/flex:\s*none/);
        expect(name).toMatch(/text-overflow:\s*ellipsis/);
        expect(name).toMatch(/min-width:\s*0/);
        expect(block(".chat-notice > svg")).toMatch(/flex:\s*none/);
    });
});
