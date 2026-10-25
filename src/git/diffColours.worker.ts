import { createHighlighterCore, type GrammarState, type HighlighterCore, type LanguageRegistration, type ThemeRegistration } from "shiki/core";
import { createOnigurumaEngine } from "shiki/dist/engine-oniguruma.mjs";
import onigWasm from "shiki/dist/onig.wasm?url";
import { codeLines } from "../chat/codeLines";
import type { CodeLine } from "@shared/chat/types";
import { errorText } from "@shared/lib/format";

export type ColourRequest =
    | { type: "grammar"; lang: string; grammars: LanguageRegistration[] }
    | { type: "theme"; theme: ThemeRegistration }
    | { type: "colour"; id: number; lang: string; theme: string; lines: string[]; maxLineLength: number }
    | { type: "cancel"; id: number };

export type ColourReply =
    { type: "lines"; id: number; from: number; lines: CodeLine[]; done: boolean } | { type: "failed"; id: number; message: string };

interface Job {
    id: number;
    lang: string;
    theme: string;
    lines: string[];
    maxLineLength: number;
    next: number;
    state?: GrammarState;
}

/* Small enough that a cancel or a newer file waits at most a few milliseconds. */
const LINES_PER_SLICE = 100;

const scope = self as unknown as { postMessage: (reply: ColourReply) => void; onmessage: ((event: MessageEvent<ColourRequest>) => void) | null };
const highlighter: Promise<HighlighterCore> = createHighlighterCore({
    themes: [],
    langs: [],
    engine: createOnigurumaEngine(fetch(onigWasm).then((response) => response.arrayBuffer())),
});
const languages = new Map<string, Promise<void>>();
const themes = new Map<string, Promise<void>>();
const jobs: Job[] = [];
let pumping = false;

scope.onmessage = ({ data }) => {
    switch (data.type) {
        case "grammar":
            if (!languages.has(data.lang))
                languages.set(
                    data.lang,
                    highlighter.then((shiki) => shiki.loadLanguage(...data.grammars)),
                );
            return;
        case "theme":
            themes.set(
                data.theme.name ?? "",
                highlighter.then((shiki) => shiki.loadTheme(data.theme)),
            );
            return;
        case "colour":
            jobs.push({ id: data.id, lang: data.lang, theme: data.theme, lines: data.lines, maxLineLength: data.maxLineLength, next: 0 });
            pump();
            return;
        case "cancel": {
            const at = jobs.findIndex((job) => job.id === data.id);
            if (at >= 0) jobs.splice(at, 1);
            return;
        }
    }
};

/** Colours the newest file first, a slice at a time, since the newest is the one that just came into view. */
function pump(): void {
    if (pumping) return;
    pumping = true;
    void (async () => {
        while (jobs.length > 0) {
            const job = jobs[jobs.length - 1];
            try {
                await Promise.all([highlighter, languages.get(job.lang), themes.get(job.theme)]);
                if (jobs[jobs.length - 1] !== job) continue;
                const shiki = await highlighter;
                const from = job.next;
                const highlighted = shiki.codeToTokens(job.lines.slice(from, from + LINES_PER_SLICE).join("\n"), {
                    lang: job.lang,
                    theme: job.theme,
                    grammarState: job.state,
                    tokenizeMaxLineLength: job.maxLineLength,
                });
                job.state = highlighted.grammarState;
                job.next = from + LINES_PER_SLICE;
                const done = job.next >= job.lines.length;
                if (done) jobs.splice(jobs.indexOf(job), 1);
                scope.postMessage({ type: "lines", id: job.id, from, lines: codeLines(highlighted), done });
            } catch (error) {
                const at = jobs.indexOf(job);
                if (at >= 0) jobs.splice(at, 1);
                scope.postMessage({ type: "failed", id: job.id, message: errorText(error) });
            }
            await new Promise((resume) => setTimeout(resume));
        }
        pumping = false;
    })();
}
