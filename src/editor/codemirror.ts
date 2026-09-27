import { StreamLanguage, type StreamParser } from "@codemirror/language";
import { Compartment, type Extension } from "@codemirror/state";
import { themeCompartmentExtension } from "./themeBridge";
import { hcl, makefile, sshConfig } from "./langs";
import { grammarFor } from "../languages";
import { tags as t } from "@lezer/highlight";

export type EditorLanguageHint = "ssh-config";

/** Every document's grammar sits here so it can be swapped in once its pack has downloaded. */
export const languageCompartment = new Compartment();

const NO_LANGUAGE: Extension[] = [];

function stream(parser: StreamParser<unknown>): Extension[] {
    return [StreamLanguage.define(parser)];
}

function legacyMode(load: () => Promise<StreamParser<unknown>>): () => Promise<Extension[]> {
    return async () => stream(await load());
}

const LANGUAGE_LOADERS: Record<string, () => Promise<Extension[]>> = {
    "ssh-config": async () => stream(sshConfig),
    makefile: async () => stream(makefile),
    hcl: async () => stream(hcl),
    dockerfile: legacyMode(async () => (await import("@codemirror/legacy-modes/mode/dockerfile")).dockerFile),
    properties: async () => {
        const { properties } = await import("@codemirror/legacy-modes/mode/properties");
        return stream({ ...properties, languageData: { commentTokens: { line: "#" } }, tokenTable: { quote: t.string } });
    },
    toml: legacyMode(async () => (await import("@codemirror/legacy-modes/mode/toml")).toml),
    shell: legacyMode(async () => (await import("@codemirror/legacy-modes/mode/shell")).shell),
    ruby: legacyMode(async () => (await import("@codemirror/legacy-modes/mode/ruby")).ruby),
    lua: legacyMode(async () => (await import("@codemirror/legacy-modes/mode/lua")).lua),
    nginx: legacyMode(async () => (await import("@codemirror/legacy-modes/mode/nginx")).nginx),
    c: legacyMode(async () => (await import("@codemirror/legacy-modes/mode/clike")).c),
    cpp: legacyMode(async () => (await import("@codemirror/legacy-modes/mode/clike")).cpp),
    java: legacyMode(async () => (await import("@codemirror/legacy-modes/mode/clike")).java),
    typescript: async () => [(await import("@codemirror/lang-javascript")).javascript({ typescript: true })],
    tsx: async () => [(await import("@codemirror/lang-javascript")).javascript({ typescript: true, jsx: true })],
    javascript: async () => [(await import("@codemirror/lang-javascript")).javascript()],
    jsx: async () => [(await import("@codemirror/lang-javascript")).javascript({ jsx: true })],
    rust: async () => [(await import("@codemirror/lang-rust")).rust()],
    python: async () => [(await import("@codemirror/lang-python")).python()],
    go: async () => [(await import("@codemirror/lang-go")).go()],
    json: async () => [(await import("@codemirror/lang-json")).json()],
    yaml: async () => [(await import("@codemirror/lang-yaml")).yaml()],
    css: async () => [(await import("@codemirror/lang-css")).css()],
    html: async () => [(await import("@codemirror/lang-html")).html()],
    markdown: async () => [(await import("@codemirror/lang-markdown")).markdown()],
};

const loadedLanguages = new Map<string, Extension[]>();
const loadingLanguages = new Map<string, Promise<Extension[]>>();

export function isSshConfigPath(path: string): boolean {
    const file = path.split(/[\\/]/).pop()?.toLowerCase() ?? "";
    return file === "ssh_config" || /(?:^|[\\/])\.ssh[\\/]config$/i.test(path);
}

export function languageIdFor(path: string, hint?: EditorLanguageHint): string | null {
    const file = path.split(/[\\/]/).pop()?.toLowerCase() ?? "";
    if (hint === "ssh-config") return "ssh-config";
    if (file === "makefile" || file === "gnumakefile" || file.endsWith(".mk")) return "makefile";
    if (file === "dockerfile" || file.startsWith("dockerfile.")) return "dockerfile";
    // dotenv: .env, .env.local, .env.production, .env.example, etc.
    if (file === ".env" || file.startsWith(".env.")) return "properties";
    if (isSshConfigPath(path)) return "ssh-config";

    const ext = file.includes(".") ? file.split(".").pop()! : "";
    switch (ext) {
        case "ts":
        case "mts":
        case "cts":
            return "typescript";
        case "tsx":
            return "tsx";
        case "js":
        case "mjs":
        case "cjs":
            return "javascript";
        case "jsx":
            return "jsx";
        case "rs":
            return "rust";
        case "py":
            return "python";
        case "go":
            return "go";
        case "json":
            return "json";
        case "yaml":
        case "yml":
            return "yaml";
        case "toml":
            return "toml";
        case "tf":
        case "tfvars":
        case "hcl":
            return "hcl";
        case "sh":
        case "bash":
        case "zsh":
            return "shell";
        case "rb":
            return "ruby";
        case "lua":
            return "lua";
        case "c":
        case "h":
            return "c";
        case "cc":
        case "cpp":
        case "cxx":
        case "hpp":
            return "cpp";
        case "java":
            return "java";
        case "css":
        case "scss":
        case "less":
            return "css";
        case "html":
        case "htm":
            return "html";
        case "md":
        case "markdown":
            return "markdown";
        case "conf":
            return "nginx";
        case "ini":
        case "env":
        case "properties":
            return "properties";
        default:
            return null;
    }
}

/** The grammar for a path if its pack is already in memory; otherwise nothing, until {@link loadLanguage} settles. */
const TEXTMATE = "textmate:";

/** The editor's own language for a path, else the grammar diffs and chat colour it with. */
function editorLanguageFor(path: string, hint?: EditorLanguageHint): string | null {
    const id = languageIdFor(path, hint);
    if (id) return id;
    const grammar = grammarFor(path);
    return grammar ? `${TEXTMATE}${grammar}` : null;
}

function loaderFor(id: string): Promise<Extension[]> {
    if (!id.startsWith(TEXTMATE)) return LANGUAGE_LOADERS[id]();
    return import("./textmate").then((module) => module.textMateLanguage(id.slice(TEXTMATE.length)));
}

export function languageFor(path: string, hint?: EditorLanguageHint): Extension[] {
    const id = editorLanguageFor(path, hint);
    return (id && loadedLanguages.get(id)) || NO_LANGUAGE;
}

/** Download a path's grammar once and keep it for every later document in that language. */
export function loadLanguage(path: string, hint?: EditorLanguageHint): Promise<Extension[]> {
    const id = editorLanguageFor(path, hint);
    if (!id) return Promise.resolve(NO_LANGUAGE);
    const ready = loadedLanguages.get(id);
    if (ready) return Promise.resolve(ready);
    let pending = loadingLanguages.get(id);
    if (!pending) {
        pending = loaderFor(id).then(
            (extensions) => {
                loadedLanguages.set(id, extensions);
                loadingLanguages.delete(id);
                return extensions;
            },
            (err: unknown) => {
                loadingLanguages.delete(id);
                throw err;
            },
        );
        loadingLanguages.set(id, pending);
    }
    return pending;
}

export const auraExtensions: Extension = themeCompartmentExtension();
export const editorThemeOnlyExtensions = (): Extension => themeCompartmentExtension({ indentMarkers: false });

/**
 * Docs larger than this skip the per-change / high-frequency extensions
 * (git diff gutter, LSP hover-link mousemove). CodeMirror itself virtualizes
 * the viewport fine; these are the extensions that do work proportional to the
 * whole document or fire on every mouse move.
 */
export const LARGE_DOC_BYTES = 256 * 1024;

export function isLargeDoc(content: string): boolean {
    return content.length > LARGE_DOC_BYTES;
}
