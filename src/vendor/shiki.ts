import { codeToHtml, createCssVariablesTheme, createHighlighterCore, getTokenStyleObject, stringifyTokenStyle } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";
import { createOnigurumaEngine } from "./oniguruma";

export const bundledLanguages = {
    c: () => import("@shikijs/langs/c"),
    css: () => import("@shikijs/langs/css"),
    go: () => import("@shikijs/langs/go"),
    html: () => import("@shikijs/langs/html"),
    java: () => import("@shikijs/langs/java"),
    json: () => import("@shikijs/langs/json"),
    jsonc: () => import("@shikijs/langs/jsonc"),
    markdown: () => import("@shikijs/langs/markdown"),
    python: () => import("@shikijs/langs/python"),
    rust: () => import("@shikijs/langs/rust"),
    shellscript: () => import("@shikijs/langs/shellscript"),
    sql: () => import("@shikijs/langs/sql"),
    typescript: () => import("@shikijs/langs/typescript"),
    yaml: () => import("@shikijs/langs/yaml"),
    zsh: () => import("@shikijs/langs/shellscript"),
};

export function createHighlighter(options: Parameters<typeof createHighlighterCore>[0]) {
    return createHighlighterCore({ ...options, langs: [] });
}

export { codeToHtml, createCssVariablesTheme, createJavaScriptRegexEngine, createOnigurumaEngine, getTokenStyleObject, stringifyTokenStyle };
