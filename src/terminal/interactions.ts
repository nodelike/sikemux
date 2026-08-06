import type { IBuffer } from "@xterm/xterm";

const MAX_TERMINAL_TITLE_CHARS = 120;

export interface TerminalSearchOptions {
    caseSensitive: boolean;
    regex: boolean;
    wholeWord: boolean;
}

/** Terminal titles are process-controlled, so never forward control bytes or unbounded text to UI state. */
export function sanitizeTerminalTitle(raw: string): string | null {
    const withoutControls = Array.from(raw, (char) => {
        const code = char.charCodeAt(0);
        return code <= 0x1f || (code >= 0x7f && code <= 0x9f) ? " " : char;
    }).join("");
    const clean = withoutControls.replace(/\s+/g, " ").trim();
    if (!clean) return null;
    const chars = Array.from(clean);
    return chars.length <= MAX_TERMINAL_TITLE_CHARS ? clean : `${chars.slice(0, MAX_TERMINAL_TITLE_CHARS - 1).join("")}…`;
}

/** Accept only credential-free HTTP(S) links; Rust's `open_url` validates again before opening. */
export function safeWebUrl(raw: string): string | null {
    try {
        const url = new URL(raw);
        if ((url.protocol !== "http:" && url.protocol !== "https:") || url.username || url.password) return null;
        return url.href;
    } catch {
        return null;
    }
}

/** Produce readable text from the active xterm buffer while preserving soft-wrapped lines. */
export function terminalBufferText(buffer: Pick<IBuffer, "length" | "getLine">): string {
    const out: string[] = [];
    let current = "";
    let started = false;
    for (let row = 0; row < buffer.length; row++) {
        const line = buffer.getLine(row);
        if (!line) continue;
        const text = line.translateToString(true);
        if (!line.isWrapped) {
            if (started) out.push(current);
            current = text;
            started = true;
        } else {
            current += text;
            started = true;
        }
    }
    if (started) out.push(current);
    while (out.length > 0 && out[out.length - 1] === "") out.pop();
    return out.join("\n");
}

export function isTerminalFindShortcut(event: Pick<KeyboardEvent, "code" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">, macos: boolean): boolean {
    return event.code === "KeyF" && !event.altKey && !event.shiftKey && (macos ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey);
}
