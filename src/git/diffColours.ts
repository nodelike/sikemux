import type { CodeLine } from "../chat/types";
import type { Theme } from "../themes";
import { createCodeTheme } from "../themes/codeTheme";
import type { ColourReply, ColourRequest } from "./diffColours.worker";

/** Colours for a run of lines, filled in as the worker gets through them; a line not reached yet is undefined. */
export type Colours = readonly (CodeLine | undefined)[];

interface Job {
    id: number;
    key: string;
    colours: (CodeLine | undefined)[];
    listeners: Set<(colours: Colours) => void>;
}

const CACHE_MAX_CHARS = 4 * 1024 * 1024;

const finished = new Map<string, CodeLine[]>();
let finishedChars = 0;
const running = new Map<string, Job>();
const runningById = new Map<number, Job>();
const sentGrammars = new Map<string, Promise<boolean>>();
const sentThemes = new Set<string>();
let worker: Worker | null = null;
let nextId = 1;

function keyOf(lines: readonly string[], lang: string, themeName: string): string {
    return `${lang}\0${themeName}\0${lines.join("\n")}`;
}

function remember(key: string, colours: CodeLine[]): void {
    finished.set(key, colours);
    finishedChars += key.length;
    for (const [old] of finished) {
        if (finishedChars <= CACHE_MAX_CHARS) break;
        finished.delete(old);
        finishedChars -= old.length;
    }
}

function post(request: ColourRequest): void {
    worker ??= startWorker();
    worker.postMessage(request);
}

function startWorker(): Worker {
    const started = new Worker(new URL("./diffColours.worker.ts", import.meta.url), { type: "module" });
    started.onmessage = ({ data }: MessageEvent<ColourReply>) => {
        const job = runningById.get(data.id);
        if (!job) return;
        if (data.type === "failed") {
            running.delete(job.key);
            runningById.delete(job.id);
            return;
        }
        const colours = job.colours.slice();
        for (let at = 0; at < data.lines.length; at++) colours[data.from + at] = data.lines[at];
        job.colours = colours;
        if (data.done) {
            running.delete(job.key);
            runningById.delete(job.id);
            remember(job.key, colours as CodeLine[]);
        }
        for (const listener of job.listeners) listener(colours);
    };
    return started;
}

function sendGrammar(lang: string): Promise<boolean> {
    let sent = sentGrammars.get(lang);
    if (!sent) {
        sent = import("../vendor/shiki").then(async ({ bundledLanguages }) => {
            const load = Object.hasOwn(bundledLanguages, lang) ? bundledLanguages[lang] : undefined;
            if (!load) return false;
            post({ type: "grammar", lang, grammars: (await load()).default });
            return true;
        });
        sentGrammars.set(lang, sent);
        sent.catch(() => sentGrammars.delete(lang));
    }
    return sent;
}

/** The colours already worked out for exactly these lines, if any. */
export function finishedColours(lines: readonly string[], lang: string, themeName: string): Colours | undefined {
    return finished.get(keyOf(lines, lang, themeName));
}

/**
 * Colours lines in a worker, off the main thread, calling back as each slice
 * arrives. Asking twice for the same lines shares the one run. Returns a stop
 * that drops the listener, and the run once nobody is listening.
 */
export function colourLines(
    lines: readonly string[],
    lang: string,
    theme: Theme,
    themeName: string,
    maxLineLength: number,
    listener: (colours: Colours) => void,
): () => void {
    const key = keyOf(lines, lang, themeName);
    const done = finished.get(key);
    if (done) {
        listener(done);
        return () => {};
    }
    let job = running.get(key);
    if (!job) {
        const started: Job = { id: nextId++, key, colours: [], listeners: new Set() };
        job = started;
        running.set(key, started);
        runningById.set(started.id, started);
        void sendGrammar(lang).then((found) => {
            if (!found || running.get(key) !== started) {
                running.delete(key);
                runningById.delete(started.id);
                return;
            }
            if (!sentThemes.has(themeName)) {
                sentThemes.add(themeName);
                post({ type: "theme", theme: createCodeTheme(theme, themeName) });
            }
            post({ type: "colour", id: started.id, lang, theme: themeName, lines: [...lines], maxLineLength });
        });
    } else if (job.colours.length > 0) {
        listener(job.colours);
    }
    const owner = job;
    owner.listeners.add(listener);
    return () => {
        owner.listeners.delete(listener);
        if (owner.listeners.size > 0 || running.get(key) !== owner) return;
        running.delete(key);
        runningById.delete(owner.id);
        post({ type: "cancel", id: owner.id });
    };
}
