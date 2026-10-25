import {
  bundledLanguages,
  createHighlighter,
  createJavaScriptRegexEngine,
} from "./vendor/shiki";
import type { Grammar, HighlighterCore } from "shiki/core";
import { createCodeTheme } from "./themes/codeTheme";
import type { Theme } from "./themes/index";
import { codeLines } from "./codeLines";
import type { CodeLine } from "@shared/chat/types";

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
const coloured = new Set<string>();

/* A grammar compiles its patterns the first time it colours anything, and on a
   busy machine that alone can outlast Shiki's limit for one line, which cuts
   the line and paints the rest in whatever colour it stopped on. The limit is
   there for slow lines, not for that, so the first use goes without it. */
function timeLimitFor(lang: string): number | undefined {
  if (coloured.has(lang)) return undefined;
  coloured.add(lang);
  return 0;
}

function highlighter(): Promise<HighlighterCore> {
  core ??= createHighlighter({
    themes: [],
    engine: createJavaScriptRegexEngine({ forgiving: true }),
    warnings: false,
  });
  return core;
}

/** Loads a grammar into the shared highlighter; false when there is no such grammar. */
async function loadGrammar(
  shiki: HighlighterCore,
  lang: string,
): Promise<boolean> {
  const grammar = Object.hasOwn(bundledLanguages, lang)
    ? bundledLanguages[lang]
    : undefined;
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

async function prepare(
  lang: string,
  theme: Theme,
  themeName: string,
): Promise<HighlighterCore | null> {
  const shiki = await highlighter();
  let loadingTheme = themes.get(themeName);
  if (!loadingTheme) {
    loadingTheme = shiki.loadTheme(createCodeTheme(theme, themeName));
    themes.set(themeName, loadingTheme);
  }
  const [, found] = await Promise.all([loadingTheme, loadGrammar(shiki, lang)]);
  return found ? shiki : null;
}

export async function tokenizeCode(
  text: string,
  lang: string,
  theme: Theme,
  themeName: string,
): Promise<CodeLine[]> {
  const shiki = await prepare(lang, theme, themeName);
  return shiki
    ? codeLines(
        shiki.codeToTokens(text, {
          lang,
          theme: themeName,
          tokenizeTimeLimit: timeLimitFor(lang),
        }),
      )
    : [];
}
