import { bundledLanguages, createHighlighter, createJavaScriptRegexEngine } from "../vendor/shiki";
import type { Grammar, GrammarState, HighlighterCore, TokensResult } from "shiki/core";
import { createCodeTheme } from "../themes/codeTheme";
import type { Theme } from "../themes";
import type { CodeLine, CodeToken } from "./types";

/* The half of the chat highlighter that carries Shiki with it. Nothing reaches
   it until a fence with a grammar we have is on screen, so a session that never
   shows code never loads it. */

/* Oniguruma reads the grammars as written, and downloads a WebAssembly engine
   to do it. The engine already in the bundle is close enough for a fence in a
   chat, and `forgiving` keeps a rule it cannot express from throwing out the
   whole file. */
let core: Promise<HighlighterCore> | null = null;
const grammars = new Map<string, Promise<unknown>>();
const themes = new Map<string, Promise<unknown>>();

function highlighter(): Promise<HighlighterCore> {
    core ??= createHighlighter({ themes: [], engine: createJavaScriptRegexEngine({ forgiving: true }), warnings: false });
    return core;
}

const FONT_ITALIC = 1;
const FONT_BOLD = 2;
const FONT_UNDERLINE = 4;

/** Loads a grammar into the shared highlighter; false when there is no such grammar. */
async function loadGrammar(shiki: HighlighterCore, lang: string): Promise<boolean> {
    const grammar = Object.hasOwn(bundledLanguages, lang) ? bundledLanguages[lang] : undefined;
    if (!grammar) return false;
    let loading = grammars.get(lang);
    if (!loading) {
        loading = grammar().then((module) => shiki.loadLanguage(module.default));
        grammars.set(lang, loading);
        loading.catch(() => grammars.delete(lang));
    }
    await loading;
    return true;
}

/** The grammar itself, for reading a file one line at a time. */
export async function textMateGrammar(lang: string): Promise<Grammar | null> {
    const shiki = await highlighter();
    return (await loadGrammar(shiki, lang)) ? shiki.getLanguage(lang) : null;
}

async function prepare(lang: string, theme: Theme, themeName: string): Promise<HighlighterCore | null> {
    const shiki = await highlighter();
    let loadingTheme = themes.get(themeName);
    if (!loadingTheme) {
        loadingTheme = shiki.loadTheme(createCodeTheme(theme, themeName));
        themes.set(themeName, loadingTheme);
    }
    const [, found] = await Promise.all([loadingTheme, loadGrammar(shiki, lang)]);
    return found ? shiki : null;
}

function codeLines(highlighted: TokensResult): CodeLine[] {
    const plain = highlighted.fg?.toLowerCase();
    return highlighted.tokens.map((line) => {
        const merged: CodeToken[] = [];
        for (const token of line) {
            const style = token.fontStyle ?? 0;
            const color = token.color && token.color.toLowerCase() !== plain ? token.color : undefined;
            const next: CodeToken = {
                text: token.content,
                ...(color ? { color } : {}),
                ...(style & FONT_ITALIC ? { italic: true } : {}),
                ...(style & FONT_BOLD ? { bold: true } : {}),
                ...(style & FONT_UNDERLINE ? { underline: true } : {}),
            };
            // A line is mostly one colour in runs, and every run that stays one
            // span is one element fewer in a list that is already long.
            const last = merged[merged.length - 1];
            if (last && sameStyle(last, next)) merged[merged.length - 1] = { ...last, text: last.text + next.text };
            else merged.push(next);
        }
        return merged;
    });
}

export async function tokenizeCode(text: string, lang: string, theme: Theme, themeName: string): Promise<CodeLine[]> {
    const shiki = await prepare(lang, theme, themeName);
    return shiki ? codeLines(shiki.codeToTokens(text, { lang, theme: themeName })) : [];
}

const LINES_PER_SLICE = 200;

/**
 * Colours a long run of lines a slice at a time, handing the main thread back
 * between slices so a big diff never holds up a frame for long. Null when
 * there is no grammar for the language or `stale` says the answer is no
 * longer wanted.
 */
export async function tokenizeLines(
    lines: readonly string[],
    lang: string,
    theme: Theme,
    themeName: string,
    { maxLineLength, stale }: { maxLineLength: number; stale: () => boolean },
): Promise<CodeLine[] | null> {
    const shiki = await prepare(lang, theme, themeName);
    if (!shiki) return null;
    const out: CodeLine[] = [];
    let grammarState: GrammarState | undefined;
    for (let from = 0; from < lines.length; from += LINES_PER_SLICE) {
        if (stale()) return null;
        const slice = lines.slice(from, from + LINES_PER_SLICE).join("\n");
        const highlighted = shiki.codeToTokens(slice, { lang, theme: themeName, grammarState, tokenizeMaxLineLength: maxLineLength });
        grammarState = highlighted.grammarState;
        out.push(...codeLines(highlighted));
        if (from + LINES_PER_SLICE < lines.length) await new Promise((resume) => setTimeout(resume));
    }
    return stale() ? null : out;
}

function sameStyle(a: CodeToken, b: CodeToken): boolean {
    return a.color === b.color && a.italic === b.italic && a.bold === b.bold && a.underline === b.underline;
}
