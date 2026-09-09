import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { preloadHighlighter, type FileContents, type FileDiffOptions } from "@pierre/diffs";
import { Editor, type EditorOptions } from "@pierre/diffs/edit";
import { EditProvider, MultiFileDiff } from "@pierre/diffs/react";
import { git } from "../api/git";
import { fsapi } from "../api/fs";
import type { Theme } from "../themes";
import { currentTheme, subscribeTheme } from "../themes/bus";
import { diffsThemeName } from "../themes/diffs";
import { errMessage, swallow } from "../state/toast";
import { joinPath } from "../lib/paths";
import { subscribe } from "../state/bus";

const DIFF_SURFACE_STYLE = {
    "--diffs-bg": "color-mix(in srgb, var(--bg) calc(var(--window-opacity, 1) * 100%), transparent)",
    "--diffs-fg": "var(--ink)",
    "--diffs-fg-number-override": "var(--ink-faint)",
    "--diffs-addition-color-override": "var(--live)",
    "--diffs-deletion-color-override": "var(--danger)",
    "--diffs-modified-color-override": "var(--acc)",
    "--diffs-bg-context-override": "color-mix(in srgb, var(--rail-2) calc(var(--window-opacity, 1) * 88%), transparent)",
    "--diffs-bg-context-gutter-override": "color-mix(in srgb, var(--rail) calc(var(--window-opacity, 1) * 55%), transparent)",
    "--diffs-bg-separator-override": "color-mix(in srgb, var(--rail-2) calc(var(--window-opacity, 1) * 92%), transparent)",
    "--diffs-font-family": "var(--mono)",
    "--diffs-header-font-family": "var(--mono)",
    "--diffs-font-size": "12px",
    "--diffs-line-height": "19px",
    width: "100%",
    minHeight: "100%",
    userSelect: "text",
} as CSSProperties;

const DIFF_UNSAFE_CSS = `
:host { background: transparent; }
[data-code] { scrollbar-color: var(--ink-faint) transparent; }
[data-code]::-webkit-scrollbar { width: 12px; height: 12px; }
[data-code]::-webkit-scrollbar-track, [data-code]::-webkit-scrollbar-corner { background: transparent; }
[data-code]::-webkit-scrollbar-thumb { background: var(--ink-faint); border: 3px solid transparent; background-clip: padding-box; }
[data-code]::-webkit-scrollbar-thumb:hover { background: var(--ink-dim); background-clip: padding-box; }
[contenteditable="true"] { caret-color: var(--acc); outline: none; }
::selection { background: var(--acc-soft); }
`;

export const DIFF_TOKENIZE_MAX_LINES = 4000;
export const DIFF_WORD_MAX_LENGTH = 512;

function createEditor(options: EditorOptions<undefined>) {
    return new Editor(options);
}

interface CachedDiffRead {
    promise: Promise<string>;
    settled: boolean;
    chars: number;
}

const revisionReads = new Map<string, CachedDiffRead>();
const DIFF_READ_CACHE_MAX_ENTRIES = 192;
const DIFF_READ_CACHE_MAX_CHARS = 32 * 1024 * 1024;
let revisionReadChars = 0;

function pruneRevisionReads(): void {
    if (revisionReads.size <= DIFF_READ_CACHE_MAX_ENTRIES && revisionReadChars <= DIFF_READ_CACHE_MAX_CHARS) return;
    for (const [key, entry] of revisionReads) {
        if (!entry.settled) continue;
        revisionReads.delete(key);
        revisionReadChars -= entry.chars;
        if (revisionReads.size <= DIFF_READ_CACHE_MAX_ENTRIES && revisionReadChars <= DIFF_READ_CACHE_MAX_CHARS) return;
    }
}

function cachedRead(key: string, load: () => Promise<string>): Promise<string> {
    const existing = revisionReads.get(key);
    if (existing) {
        revisionReads.delete(key);
        revisionReads.set(key, existing);
        return existing.promise;
    }

    const entry: CachedDiffRead = { promise: Promise.resolve(""), settled: false, chars: 0 };
    const pending = load().then(
        (value) => {
            if (revisionReads.get(key) === entry) {
                entry.settled = true;
                entry.chars = value.length;
                revisionReadChars += value.length;
                pruneRevisionReads();
            }
            return value;
        },
        (error: unknown) => {
            if (revisionReads.get(key) === entry) revisionReads.delete(key);
            throw error;
        },
    );
    entry.promise = pending;
    revisionReads.set(key, entry);
    pruneRevisionReads();
    return pending;
}

function readRevision(repo: string, rev: string, path: string): Promise<string> {
    const key = `${repo}\0${rev}\0${path}`;
    return cachedRead(key, () => git.fileAt(repo, rev, path));
}

export function invalidateDiffContentCache(repo?: string): void {
    if (!repo) {
        revisionReads.clear();
        revisionReadChars = 0;
        return;
    }
    const prefix = `${repo}\0`;
    for (const [key, entry] of revisionReads) {
        if (!key.startsWith(prefix)) continue;
        revisionReads.delete(key);
        if (entry.settled) revisionReadChars -= entry.chars;
    }
}

subscribe("fs-changed", (event) => {
    invalidateDiffContentCache(event.repo || undefined);
});

export function DiffEditor({
    repo,
    path,
    baseRev,
    headRev,
    editable,
    autoHeight,
    onSaved,
}: {
    repo: string;
    path: string;
    baseRev: string;
    headRev?: string;
    editable: boolean;
    autoHeight?: boolean;
    onSaved?: () => void;
}) {
    const [content, setContent] = useState<{ base: string; head: string } | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [diffTheme, setDiffTheme] = useState(() => resolveDiffTheme(currentTheme()));
    const latestHeadRef = useRef("");
    const onSavedRef = useRef(onSaved);
    onSavedRef.current = onSaved;
    const absPath = joinPath(repo, path);

    useEffect(() => subscribeTheme((theme) => setDiffTheme(resolveDiffTheme(theme))), []);

    useEffect(() => {
        void preloadHighlighter({ themes: [diffTheme.name], langs: [diffLanguage(path)] }).catch(swallow("diff renderer preload"));
    }, [diffTheme.name, path]);

    useEffect(() => {
        let cancelled = false;
        setContent(null);
        setError(null);

        void Promise.all([readRevision(repo, baseRev, path), headRev ? readRevision(repo, headRev, path) : readWorkingFile(repo, path, absPath)])
            .then(([base, head]) => {
                if (cancelled) return;
                const guard = inlineDiffGuard(path, base, head);
                if (guard) {
                    setError(guard);
                    return;
                }
                latestHeadRef.current = head;
                setContent({ base, head });
            })
            .catch((err) => {
                if (!cancelled) setError(errMessage(err));
            });

        return () => {
            cancelled = true;
        };
    }, [repo, path, baseRev, headRev, absPath]);

    const files = useMemo(() => {
        if (!content) return null;
        const baseKey = `${repo}:${baseRev}:${path}:${diffTheme.name}:${contentHash(content.base)}`;
        const headKey = `${repo}:${headRev ?? "worktree"}:${path}:${diffTheme.name}:${contentHash(content.head)}`;
        const lang = diffLanguage(path);
        return {
            oldFile: { name: path, contents: content.base, cacheKey: baseKey, lang } satisfies FileContents,
            newFile: { name: path, contents: content.head, cacheKey: headKey, lang } satisfies FileContents,
        };
    }, [content, repo, path, baseRev, headRev, diffTheme.name]);

    const options = useMemo<FileDiffOptions<undefined>>(
        () => ({
            theme: diffTheme.name,
            themeType: diffTheme.dark ? "dark" : "light",
            diffStyle: "unified",
            diffIndicators: "bars",
            disableBackground: !diffTheme.dark,
            hunkSeparators: "line-info-basic",
            lineDiffType: editable ? "word-alt" : "none",
            maxLineDiffLength: DIFF_WORD_MAX_LENGTH,
            tokenizeMaxLength: DIFF_TOKENIZE_MAX_LINES,
            collapsedContextThreshold: 1,
            expansionLineCount: 50,
            overflow: "scroll",
            disableFileHeader: true,
            unsafeCSS: DIFF_UNSAFE_CSS,
        }),
        [diffTheme, editable],
    );

    const editorOptions = useMemo<EditorOptions<undefined>>(
        () => ({
            onChange(file) {
                latestHeadRef.current = file.contents;
            },
        }),
        [],
    );

    const save = useCallback(() => {
        void fsapi
            .writeFile(absPath, latestHeadRef.current)
            .then(() => onSavedRef.current?.())
            .catch(swallow("DiffEditor save"));
    }, [absPath]);

    const onKeyDownCapture = (event: ReactKeyboardEvent<HTMLDivElement>) => {
        if (!editable || event.key.toLowerCase() !== "s" || (!event.metaKey && !event.ctrlKey)) return;
        event.preventDefault();
        event.stopPropagation();
        save();
    };

    return (
        <div className={`diff-editor${autoHeight ? " auto" : ""}`} onKeyDownCapture={onKeyDownCapture}>
            {error ? (
                <div className="diff-editor-error">x {error}</div>
            ) : files ? (
                <EditProvider createEditor={createEditor}>
                    <MultiFileDiff
                        key={diffTheme.name}
                        oldFile={files.oldFile}
                        newFile={files.newFile}
                        options={options}
                        edit={editable}
                        disableWorkerPool={editable}
                        editorOptions={editable ? editorOptions : undefined}
                        style={{ ...DIFF_SURFACE_STYLE, colorScheme: diffTheme.dark ? "dark" : "light" }}
                    />
                </EditProvider>
            ) : (
                <div className="diff-editor-loading">loading diff...</div>
            )}
        </div>
    );
}

function resolveDiffTheme(theme: Theme) {
    return { name: diffsThemeName(theme), dark: theme.dark };
}

const MAX_INLINE_DIFF_CHARS = 1024 * 1024;

function inlineDiffGuard(path: string, base: string, head: string): string | null {
    const largest = Math.max(base.length, head.length);
    if (largest > MAX_INLINE_DIFF_CHARS) return `${path} is too large for inline diff (${humanChars(largest)}).`;
    if (looksBinaryText(base) || looksBinaryText(head)) return `${path} looks binary; inline diff is disabled.`;
    return null;
}

function looksBinaryText(value: string): boolean {
    const sample = value.slice(0, 8192);
    if (sample.includes("\0")) return true;
    let replacements = 0;
    for (let i = 0; i < sample.length; i++) if (sample.charCodeAt(i) === 0xfffd) replacements++;
    return replacements > 8;
}

function humanChars(value: number): string {
    return value > 1024 * 1024 ? `${(value / 1024 / 1024).toFixed(1)} MB` : `${(value / 1024).toFixed(1)} KB`;
}

function contentHash(value: string): string {
    let result = 2166136261;
    for (let i = 0; i < value.length; i++) {
        result ^= value.charCodeAt(i);
        result = Math.imul(result, 16777619);
    }
    return (result >>> 0).toString(36);
}

function diffLanguage(path: string): NonNullable<FileContents["lang"]> {
    const name = path.split(/[\\/]/).pop()?.toLowerCase() ?? "";
    if (name === "dockerfile") return "shellscript";
    if (name === "makefile") return "shellscript";
    const extension = name.includes(".") ? name.slice(name.lastIndexOf(".") + 1) : "";
    if (["js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts"].includes(extension)) return "typescript";
    if (["css", "scss", "sass", "less"].includes(extension)) return "css";
    if (["html", "htm", "vue", "svelte"].includes(extension)) return "html";
    if (["json", "json5", "jsonc", "jsonl"].includes(extension)) return "jsonc";
    if (["md", "mdx", "markdown"].includes(extension)) return "markdown";
    if (["sh", "bash", "zsh", "fish"].includes(extension)) return "shellscript";
    if (["yaml", "yml"].includes(extension)) return "yaml";
    if (["py", "pyi", "pyw"].includes(extension)) return "python";
    if (["c", "h"].includes(extension)) return "c";
    if (extension === "rs") return "rust";
    if (extension === "go") return "go";
    if (extension === "java") return "java";
    if (extension === "sql") return "sql";
    return "text";
}

async function readWorkingFile(repo: string, path: string, absPath: string): Promise<string> {
    return cachedRead(`${repo}\0:worktree\0${path}`, async () => {
        try {
            return await fsapi.readTextFileLimited(absPath);
        } catch (err) {
            if (isMissingFileError(err)) return "";
            throw err;
        }
    });
}

function isMissingFileError(err: unknown): boolean {
    const msg = errMessage(err).toLowerCase();
    return msg.includes("no such file") || msg.includes("not found") || msg.includes("os error 2");
}
