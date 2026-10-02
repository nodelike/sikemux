import { lazy, Suspense, useCallback, useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { git, type DiffRow } from "../api/git";
import { fsapi } from "../api/fs";
import { currentTheme, subscribeTheme } from "../themes/bus";
import { errMessage, swallow } from "../state/toast";
import { joinPath } from "../lib/paths";
import { subscribe } from "../state/bus";
import { DiffView } from "./DiffView";

const DiffMergeEditor = lazy(() => import("./DiffMergeEditor"));

interface CachedDiffRead {
    promise: Promise<unknown>;
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

function cachedRead<T>(key: string, load: () => Promise<T>, charsOf: (value: T) => number): Promise<T> {
    const existing = revisionReads.get(key);
    if (existing) {
        revisionReads.delete(key);
        revisionReads.set(key, existing);
        return existing.promise as Promise<T>;
    }

    const entry: CachedDiffRead = { promise: Promise.resolve(), settled: false, chars: 0 };
    const pending = load().then(
        (value) => {
            if (revisionReads.get(key) === entry) {
                entry.settled = true;
                entry.chars = charsOf(value);
                revisionReadChars += entry.chars;
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

const textLength = (text: string) => text.length;
const rowsLength = (rows: DiffRow[]) => rows.reduce((total, row) => total + row[2].length + 8, 0);

function readRevision(repo: string, rev: string, path: string): Promise<string> {
    return cachedRead(`${repo}\0${rev}\0${path}`, () => git.fileAt(repo, rev, path), textLength);
}

function readDiff(repo: string, path: string, baseRev: string, headRev: string | undefined, full: boolean): Promise<DiffRow[]> {
    const key = `${repo}\0diff\0${baseRev}\0${headRev ?? ":worktree"}\0${path}\0${full ? "full" : "hunks"}`;
    return cachedRead(key, () => git.fileDiff(repo, path, baseRev, headRev ?? null, full), rowsLength);
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
subscribe("git-refresh", (event) => {
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
    const [rows, setRows] = useState<DiffRow[] | null>(null);
    const [files, setFiles] = useState<{ base: string; head: string } | null>(null);
    const [expanded, setExpanded] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [dark, setDark] = useState(() => currentTheme().dark);
    const latestHeadRef = useRef("");
    const onSavedRef = useRef(onSaved);
    onSavedRef.current = onSaved;
    const absPath = joinPath(repo, path);
    const diffKey = `${repo}\0${path}\0${baseRev}\0${headRev ?? ""}`;
    const full = expanded === diffKey;
    const filesRef = useRef(files);
    filesRef.current = files;
    // Bumped when the working tree or the index may have moved under this diff, so it reads again.
    const [changes, setChanges] = useState(0);
    const moving = !headRev || headRev === ":index";

    useEffect(() => {
        if (!moving) return;
        const reread = () => setChanges((n) => n + 1);
        const stopFs = subscribe("fs-changed", (event) => {
            if (event.repo && event.repo !== repo) return;
            if (!event.paths || event.paths.includes(path)) reread();
        });
        const stopGit = subscribe("git-refresh", (event) => {
            if (!event.repo || event.repo === repo) reread();
        });
        return () => {
            stopFs();
            stopGit();
        };
    }, [repo, path, moving]);

    useEffect(() => subscribeTheme((theme) => setDark(theme.dark)), []);

    useEffect(() => {
        setRows(null);
        setFiles(null);
    }, [diffKey]);

    useEffect(() => {
        if (editable) return;
        let cancelled = false;
        setError(null);
        readDiff(repo, path, baseRev, headRev, full)
            .then((next) => {
                if (!cancelled) setRows(next);
            })
            .catch((err: unknown) => {
                if (!cancelled) setError(errMessage(err));
            });
        return () => {
            cancelled = true;
        };
    }, [repo, path, baseRev, headRev, full, editable, changes]);

    useEffect(() => {
        if (!editable) return;
        // Typing that has not been saved yet wins over a change from outside.
        if (filesRef.current && latestHeadRef.current !== filesRef.current.head) return;
        let cancelled = false;
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
                setFiles((current) => (current?.base === base && current.head === head ? current : { base, head }));
            })
            .catch((err: unknown) => {
                if (!cancelled) setError(errMessage(err));
            });
        return () => {
            cancelled = true;
        };
    }, [repo, path, baseRev, headRev, absPath, editable, changes]);

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

    const loading = <div className="diff-editor-loading">loading diff...</div>;
    let body = loading;
    if (error) body = <div className="diff-editor-error">x {error}</div>;
    else if (editable && files)
        body = (
            <Suspense fallback={loading}>
                <DiffMergeEditor
                    base={files.base}
                    head={files.head}
                    path={path}
                    tinted={dark}
                    onChange={(text) => {
                        latestHeadRef.current = text;
                    }}
                />
            </Suspense>
        );
    else if (!editable && rows) body = <DiffView rows={rows} path={path} tinted={dark} onShowHidden={() => setExpanded(diffKey)} />;

    return (
        <div className={`diff-editor${autoHeight ? " auto" : ""}`} onKeyDownCapture={onKeyDownCapture}>
            {body}
        </div>
    );
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

async function readWorkingFile(repo: string, path: string, absPath: string): Promise<string> {
    return cachedRead(
        `${repo}\0:worktree\0${path}`,
        async () => {
            try {
                return await fsapi.readTextFileLimited(absPath);
            } catch (err) {
                if (isMissingFileError(err)) return "";
                throw err;
            }
        },
        textLength,
    );
}

function isMissingFileError(err: unknown): boolean {
    const msg = errMessage(err).toLowerCase();
    return msg.includes("no such file") || msg.includes("not found") || msg.includes("os error 2");
}
