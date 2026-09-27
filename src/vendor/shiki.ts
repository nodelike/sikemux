import { codeToHtml, createCssVariablesTheme, createHighlighterCore, getTokenStyleObject, stringifyTokenStyle } from "shiki/core";
import type { LanguageRegistration } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";
import { createOnigurumaEngine } from "./oniguruma";
import { invokeCommand } from "../api/invoke";
import { GRAMMARS } from "../languages/generated/grammars";

type GrammarModule = { default: LanguageRegistration[] };

/* Grammars for the languages this app is mostly used on ship with it. Every
   other grammar is downloaded the first time a file needs it, and the native
   side keeps it on disk from then on. */
const SHIPPED: Readonly<Record<string, () => Promise<GrammarModule>>> = {
    css: () => import("@shikijs/langs/css"),
    html: () => import("@shikijs/langs/html"),
    json: () => import("@shikijs/langs/json"),
    jsonc: () => import("@shikijs/langs/jsonc"),
    markdown: () => import("@shikijs/langs/markdown"),
    python: () => import("@shikijs/langs/python"),
    rust: () => import("@shikijs/langs/rust"),
    shellscript: () => import("@shikijs/langs/shellscript"),
    typescript: () => import("@shikijs/langs/typescript"),
    yaml: () => import("@shikijs/langs/yaml"),
};

async function downloaded(id: string): Promise<GrammarModule> {
    const embedded = await Promise.all(GRAMMARS[id].map((embed) => bundledLanguages[embed]()));
    const own = JSON.parse(await invokeCommand<string>("grammar_load", { id })) as LanguageRegistration;
    const seen = new Set<string>();
    const grammars = [...embedded.flatMap((module) => module.default), own].filter((grammar) => !seen.has(grammar.name) && seen.add(grammar.name));
    return { default: grammars };
}

export const bundledLanguages: Readonly<Record<string, () => Promise<GrammarModule>>> = Object.fromEntries(
    Object.keys(GRAMMARS).map((id) => [id, Object.hasOwn(SHIPPED, id) ? SHIPPED[id] : () => downloaded(id)]),
);

export function createHighlighter(options: Parameters<typeof createHighlighterCore>[0]) {
    return createHighlighterCore({ ...options, langs: [] });
}

export { codeToHtml, createCssVariablesTheme, createJavaScriptRegexEngine, createOnigurumaEngine, getTokenStyleObject, stringifyTokenStyle };
