import { coreCall } from "./core";
import { cancelCoreStream, coreStream } from "./coreStream";

export interface SearchOptions {
    caseSensitive: boolean;
    wholeWord: boolean;
    isRegex: boolean;
    include: string;
    exclude: string;
}

export interface SearchRange {
    start: number;
    end: number;
}

export interface SearchHit {
    line: number; // 1-based
    text: string;
    truncatedText?: boolean;
    ranges: SearchRange[];
}

export interface SearchFile {
    path: string; // repo-relative
    matches: SearchHit[];
}

export interface SearchResults {
    files: SearchFile[];
    fileCount: number;
    matchCount: number;
    truncated: boolean;
    cancelled?: boolean;
    elapsedMs: number;
}

export interface ReplaceFile {
    path: string;
    matchCount: number;
}

export interface ReplaceError {
    path: string;
    reason: string;
}

export interface ReplaceResults {
    files: ReplaceFile[];
    fileCount: number;
    matchCount: number;
    errors: ReplaceError[];
    dryRun: boolean;
    elapsedMs: number;
}

export interface FileWindow {
    doc: string;
    startLine: number;
    totalLines: number;
    clippedHead: boolean;
    clippedTail: boolean;
}

type SearchSummary = Omit<SearchResults, "files">;

/** The search running in each project, by project folder, so a newer one or the pane's cancel can stop it. */
const running = new Map<string, number>();

export const searchApi = {
    project: async (repo: string, query: string, options: SearchOptions, onFile: (file: SearchFile) => void): Promise<SearchResults> => {
        let summary: SearchSummary = { fileCount: 0, matchCount: 0, truncated: false, elapsedMs: 0 };
        const { stream, done } = await coreStream("searchStream", { project: repo, query, options }, (item) => {
            const part = item as { file?: SearchFile; summary?: SearchSummary };
            if (part.file) onFile(part.file);
            if (part.summary) summary = part.summary;
        });
        running.set(repo, stream);
        try {
            await done;
        } finally {
            if (running.get(repo) === stream) running.delete(repo);
        }
        return { files: [], ...summary };
    },
    cancel: async (repo: string): Promise<void> => {
        const stream = running.get(repo);
        if (stream === undefined) return;
        running.delete(repo);
        await cancelCoreStream(stream);
    },
    replace: (repo: string, query: string, replace: string, options: SearchOptions, dryRun: boolean): Promise<ReplaceResults> =>
        coreCall<ReplaceResults>("searchReplace", { project: repo, query, replace, options, dryRun }),
    readFileWindow: (path: string, line: number, before: number, after: number): Promise<FileWindow> =>
        coreCall<FileWindow>("fileWindow", { path, line, before, after }),
};
