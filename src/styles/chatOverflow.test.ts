import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const stylesDir = join(process.cwd(), "src", "styles");
const chat = [...readFileSync(join(stylesDir, "chat.css"), "utf8").matchAll(/@import\s+"\.\/([\w/-]+\.css)"/g)]
    .map((m) => readFileSync(join(stylesDir, m[1]), "utf8"))
    .join("\n");

function block(selector: string): string {
    const match = chat.match(new RegExp(`(^|\\n)${selector.replace(/[.\\-]/g, "\\$&")}\\s*\\{([^}]*)\\}`));
    expect(match, `${selector} is missing from chat.css`).not.toBeNull();
    return match?.[2] ?? "";
}

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
