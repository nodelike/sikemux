/** What the job graph needs from one entry under a workflow file's `jobs:`. */
export interface WorkflowJob {
    key: string;
    name: string | null;
    needs: string[];
    /** The workflow this job calls instead of running steps, as written after `uses:`. */
    uses: string | null;
}

interface Line {
    indent: number;
    text: string;
}

const KEY = /^([A-Za-z_][\w-]*)\s*:(?:\s+(.*))?$/;

function withoutComment(raw: string): string {
    let quote: string | null = null;
    for (let at = 0; at < raw.length; at += 1) {
        const char = raw[at];
        if (quote) {
            if (char === quote) quote = null;
        } else if ((char === '"' || char === "'") && (at === 0 || /[\s[{,]/.test(raw[at - 1] ?? ""))) {
            quote = char;
        } else if (char === "#" && (at === 0 || /\s/.test(raw[at - 1] ?? ""))) {
            return raw.slice(0, at);
        }
    }
    return raw;
}

function linesOf(text: string): Line[] {
    const lines: Line[] = [];
    for (const raw of text.split(/\r?\n/)) {
        const body = withoutComment(raw).trimEnd();
        const trimmed = body.trimStart();
        if (trimmed) lines.push({ indent: body.length - trimmed.length, text: trimmed });
    }
    return lines;
}

function scalar(value: string): string {
    const text = value.trim();
    if (text.length >= 2 && text.startsWith("'") && text.endsWith("'")) return text.slice(1, -1).replaceAll("''", "'");
    if (text.length >= 2 && text.startsWith('"') && text.endsWith('"')) {
        try {
            return JSON.parse(text) as string;
        } catch {
            return text.slice(1, -1);
        }
    }
    return text;
}

function listOf(flow: string): string[] {
    return flow
        .replace(/^\[|\]$/g, "")
        .split(",")
        .map(scalar)
        .filter(Boolean);
}

/**
 * Reads each job's key, `name`, `needs` and `uses` from a workflow file, or null when it has no `jobs:`.
 * It reads only what GitHub's own files write for those, not the whole of YAML.
 */
export function readJobs(text: string): WorkflowJob[] | null {
    const lines = linesOf(text);
    const start = lines.findIndex((line) => line.indent === 0 && /^jobs\s*:$/.test(line.text));
    if (start < 0) return null;
    const jobIndent = lines[start + 1]?.indent ?? 0;
    if (jobIndent === 0) return [];

    const jobs: WorkflowJob[] = [];
    let job: WorkflowJob | null = null;
    let fieldIndent: number | null = null;
    for (let at = start + 1; at < lines.length; at += 1) {
        const line = lines[at] as Line;
        if (line.indent < jobIndent) break;
        if (line.indent === jobIndent) {
            const key = KEY.exec(line.text)?.[1];
            job = key ? { key, name: null, needs: [], uses: null } : null;
            if (job) jobs.push(job);
            fieldIndent = null;
            continue;
        }
        if (!job) continue;
        fieldIndent ??= line.indent;
        if (line.indent !== fieldIndent) continue;
        const field = KEY.exec(line.text);
        if (!field) continue;
        const value = field[2] ?? "";
        if (field[1] === "name" && !/^[|>]/.test(value)) job.name = scalar(value) || null;
        else if (field[1] === "uses") job.uses = scalar(value) || null;
        else if (field[1] === "needs") {
            if (value.startsWith("[")) {
                let flow = value;
                while (!flow.includes("]") && at + 1 < lines.length && (lines[at + 1] as Line).indent > fieldIndent) {
                    at += 1;
                    flow += ` ${(lines[at] as Line).text}`;
                }
                job.needs = listOf(flow);
            } else if (value) {
                job.needs = [scalar(value)];
            } else {
                const needs: string[] = [];
                while (at + 1 < lines.length) {
                    const next = lines[at + 1] as Line;
                    if (next.indent < fieldIndent || !next.text.startsWith("-")) break;
                    needs.push(scalar(next.text.slice(1)));
                    at += 1;
                }
                job.needs = needs.filter(Boolean);
            }
        }
    }
    return jobs;
}

/** The repository path of a workflow called from the same repository, such as `./.github/workflows/ci.yml`. */
export function calledPath(uses: string): string | null {
    if (!uses.startsWith("./")) return null;
    return uses.slice(2).split("@")[0] ?? null;
}
