import { Fragment, useEffect, useMemo, useRef, useState, useSyncExternalStore, type CSSProperties } from "react";
import { currentTheme, subscribeTheme } from "../themes/bus";
import { codeThemeName } from "../themes/codeTheme";
import { swallow } from "../state/toast";
import { grammarFor } from "../languages";
import type { CodeLine, CodeToken } from "./types";
import type { DiffLine } from "./diff";

/** The grammar a fence asks for, whether it names a language or a file. */
export function fenceLanguage(info: string | undefined): string | null {
    return info ? grammarFor(info) : null;
}

/*
 * What a fence has to stay under to be worth colouring.
 *
 * Reading a block is one pass over its text that cannot be split across
 * frames, and it costs about what its length says it should. These are set
 * where the longest block still fits in a frame; past them is a file an agent
 * pasted rather than anything read in a column this narrow, and it stays as
 * plain as it arrived.
 */
const MAX_CHARS = 6_000;
const MAX_LINES = 150;

/*
 * How long a fence has to hold still before it is worth colouring.
 *
 * A message being written re-reads its markdown ten times a second, so the last
 * fence in it grows every 100ms while the ones above it are already finished.
 * Waiting out one of those reads means the growing block is never tokenised and
 * the settled ones colour as soon as the agent moves past them.
 */
const SETTLE_MS = 150;

/* Colouring the same fence again on every re-read, and again every time a row
   comes back on screen, would be most of the work the transcript ever does.
   Bounded both ways: a long transcript of code cannot grow this without end. */
const MAX_ENTRIES = 96;
const MAX_CACHED_CHARS = 400_000;
const cache = new Map<string, { chars: number; lines: CodeLine[] }>();
let cachedChars = 0;

function remember(key: string, chars: number, lines: CodeLine[]) {
    cache.set(key, { chars, lines });
    cachedChars += chars;
    for (const [oldest, dropped] of cache) {
        if (cache.size <= MAX_ENTRIES && cachedChars <= MAX_CACHED_CHARS) break;
        if (oldest === key) break;
        cache.delete(oldest);
        cachedChars -= dropped.chars;
    }
}

function smallEnough(text: string): boolean {
    if (!text || text.length > MAX_CHARS) return false;
    let lines = 1;
    for (let at = text.indexOf("\n"); at >= 0; at = text.indexOf("\n", at + 1)) {
        lines += 1;
        if (lines > MAX_LINES) return false;
    }
    return true;
}

/* Shiki is a megabyte the app already carries for its diff panes, and the
   grammar behind it another chunk after that. Warming it while a fence is
   still being written means the reading itself is all that is left to do when
   it settles. */
let warming: Promise<typeof import("./shikiTokens")> | null = null;

function warm() {
    warming ??= import("./shikiTokens");
    return warming;
}

async function colour(key: string, text: string, lang: string, themeName: string): Promise<void> {
    const shiki = await warm();
    const theme = currentTheme();
    // The theme moved while the highlighter was loading; the fence has asked
    // for the new one by now and this answer is for a palette nothing shows.
    if (codeThemeName(theme) !== themeName) return;
    const lines = await shiki.tokenizeCode(text, lang, theme, themeName);
    if (lines.length > 0) remember(key, text.length, lines);
}

const themeNameNow = () => codeThemeName(currentTheme());

function useCodeThemeName(): string {
    return useSyncExternalStore(subscribeTheme, themeNameNow, themeNameNow);
}

/**
 * The colours for a fence, or null while there are none to show: a grammar we
 * do not have, a block too big to be worth it, or one that is still being
 * written. Reading is synchronous and cached, so a row that comes back on
 * screen paints coloured on its first frame.
 */
export function useCodeTokens(text: string, lang: string | null): CodeLine[] | null {
    const themeName = useCodeThemeName();
    const key = lang && smallEnough(text) ? `${themeName}\u0000${lang}\u0000${text}` : null;
    const [, redraw] = useState(0);
    const kept = useRef<{ text: string; lines: CodeLine[] } | null>(null);

    useEffect(() => {
        if (!lang) return;
        // Even a fence that is too long, or still growing, says this session
        // is one that will want the highlighter.
        void warm();
        if (!key || cache.has(key)) return;
        let listening = true;
        const timer = window.setTimeout(() => {
            void colour(key, text, lang, themeName)
                .then(() => {
                    if (listening) redraw((count) => count + 1);
                })
                .catch(swallow("colour a code fence"));
        }, SETTLE_MS);
        return () => {
            listening = false;
            window.clearTimeout(timer);
        };
    }, [key, lang, text, themeName]);

    const lines = (key && cache.get(key)?.lines) || null;
    if (lines) kept.current = { text, lines };
    // A theme change asks for the same code in new colours. Holding the old
    // ones until they arrive keeps the fence from blinking back to plain text,
    // which the text changing cannot do: those colours are for other words.
    return lines ?? (kept.current?.text === text ? kept.current.lines : null);
}

/* The two files a diff is a reading of. Each one is a real slice of a real
   file, which the two of them interleaved is not: a deleted line and the line
   that replaced it cannot both be there. */
function sidesOf(lines: readonly DiffLine[]): { before: string; after: string } {
    const before: string[] = [];
    const after: string[] = [];
    for (const line of lines) {
        if (line.sign !== "+") before.push(line.text);
        if (line.sign !== "-") after.push(line.text);
    }
    return { before: before.join("\n"), after: after.join("\n") };
}

function sideBySide(lines: readonly DiffLine[], before: CodeLine[] | null, after: CodeLine[] | null): Map<DiffLine, CodeLine> | null {
    if (!before && !after) return null;
    const coloured = new Map<DiffLine, CodeLine>();
    let deleted = 0;
    let added = 0;
    for (const line of lines) {
        // A line the change left alone reads the same either way, so it takes
        // whichever side has come back.
        const colours = line.sign === "-" ? before?.[deleted] : (after?.[added] ?? (line.sign === " " ? before?.[deleted] : undefined));
        if (colours) coloured.set(line, colours);
        if (line.sign !== "+") deleted += 1;
        if (line.sign !== "-") added += 1;
    }
    return coloured.size > 0 ? coloured : null;
}

const NO_LINES: readonly DiffLine[] = [];

/**
 * The colours for the lines of a diff, by the line they belong to. Each side is
 * read as the file it came from, so a deleted line is coloured by the file it
 * was deleted from rather than by the one that replaced it.
 */
export function useDiffTokens(lines: readonly DiffLine[] | null, path: string | undefined): Map<DiffLine, CodeLine> | null {
    const rows = lines ?? NO_LINES;
    const lang = useMemo(() => fenceLanguage(path), [path]);
    const sides = useMemo(() => sidesOf(rows), [rows]);
    const before = useCodeTokens(sides.before, lang);
    const after = useCodeTokens(sides.after, lang);
    return useMemo(() => sideBySide(rows, before, after), [rows, before, after]);
}

function sliced(token: CodeToken, from: number, to: number): CodeToken | null {
    const text = token.text.slice(from, to);
    return text ? { ...token, text } : null;
}

/**
 * A line's runs, with the changed span lifted out of them. One mark has to wrap
 * the whole span — it is a rounded box, and a row of them is not the same thing
 * — so the runs it crosses are cut at its edges rather than wrapped one by one.
 */
export function splitAtMark(tokens: CodeLine, mark?: readonly [number, number]): { pre: CodeLine; marked: CodeLine; post: CodeLine } {
    if (!mark) return { pre: tokens, marked: [], post: [] };
    const [from, to] = mark;
    const pre: CodeToken[] = [];
    const marked: CodeToken[] = [];
    const post: CodeToken[] = [];
    let at = 0;
    for (const token of tokens) {
        const end = at + token.text.length;
        const piece = (start: number, stop: number) => sliced(token, Math.max(start, at) - at, Math.min(stop, end) - at);
        const head = at < from ? piece(0, from) : null;
        const middle = end > from && at < to ? piece(from, to) : null;
        const tail = end > to ? piece(to, end) : null;
        if (head) pre.push(head);
        if (middle) marked.push(middle);
        if (tail) post.push(tail);
        at = end;
    }
    return { pre, marked, post };
}

function styleOf(token: CodeToken): CSSProperties | undefined {
    if (!token.color && !token.italic && !token.bold && !token.underline) return undefined;
    return {
        color: token.color,
        fontStyle: token.italic ? "italic" : undefined,
        fontWeight: token.bold ? 600 : undefined,
        textDecoration: token.underline ? "underline" : undefined,
    };
}

/** One line's runs. A run with no colour of its own stays a plain text node. */
export function CodeRun({ tokens }: { tokens: CodeLine }) {
    return (
        <>
            {tokens.map((token, at) => {
                const style = styleOf(token);
                return style ? (
                    <span key={at} style={style}>
                        {token.text}
                    </span>
                ) : (
                    token.text
                );
            })}
        </>
    );
}

/** A fence's text, coloured. The characters are the fence's own, unchanged. */
export function CodeTokens({ lines }: { lines: CodeLine[] }) {
    return (
        <>
            {lines.map((line, index) => (
                <Fragment key={index}>
                    {index > 0 && "\n"}
                    <CodeRun tokens={line} />
                </Fragment>
            ))}
        </>
    );
}
