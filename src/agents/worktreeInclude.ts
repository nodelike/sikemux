import { fsapi } from "../api/fs";
import { joinPath } from "../lib/paths";

/** Without a `.worktreeinclude`, the files a project usually keeps out of Git but needs to run. */
const DEFAULT_INCLUDE = [".env", ".env.*", "*/.env", "*/.env.*"];
const MAX_COPIED = 50;

function segmentPattern(segment: string): RegExp {
    return new RegExp(`^${segment.replace(/[.+?^${}()|[\]\\]/g, "\\$&").replace(/\*/g, "[^/]*")}$`);
}

async function expand(root: string, pattern: string): Promise<string[]> {
    const segments = pattern.replace(/^\/+/, "").split("/").filter(Boolean);
    let paths = [""];
    for (const [index, segment] of segments.entries()) {
        const last = index === segments.length - 1;
        const next: string[] = [];
        for (const base of paths) {
            if (!segment.includes("*")) {
                next.push(base ? `${base}/${segment}` : segment);
                continue;
            }
            const entries = await fsapi.readDir(joinPath(root, base)).catch(() => []);
            const match = segmentPattern(segment);
            for (const entry of entries) if (match.test(entry.name) && (last || entry.is_dir)) next.push(base ? `${base}/${entry.name}` : entry.name);
        }
        paths = next;
    }
    return paths;
}

/**
 * Copies the files a fresh checkout is missing but needs to run, such as `.env`,
 * from the main checkout into a new worktree. Lists them in `.worktreeinclude`
 * when the project has one, the same file Claude Code and Codex read.
 */
export async function copyWorktreeIncludes(main: string, worktree: string): Promise<string[]> {
    const listed = await fsapi.readTextFileLimited(joinPath(main, ".worktreeinclude")).catch(() => null);
    const patterns =
        listed === null
            ? DEFAULT_INCLUDE
            : listed
                  .split("\n")
                  .map((line) => line.trim())
                  .filter((line) => line && !line.startsWith("#"));
    const relative = [...new Set((await Promise.all(patterns.map((pattern) => expand(main, pattern)))).flat())].slice(0, MAX_COPIED);
    const sources = relative.map((path) => joinPath(main, path));
    const targets = relative.map((path) => joinPath(worktree, path));
    const targetDirs = relative.map((path) => joinPath(worktree, path.split("/").slice(0, -1).join("/")));
    const [sourceKinds, targetKinds, dirKinds] = await Promise.all([fsapi.pathKinds(sources), fsapi.pathKinds(targets), fsapi.pathKinds(targetDirs)]);
    const copied: string[] = [];
    for (const [index, path] of relative.entries()) {
        if (sourceKinds[index] !== "file" || targetKinds[index] !== null || dirKinds[index] !== "dir") continue;
        await fsapi.copyIntoDir(sources[index], targetDirs[index]);
        copied.push(path);
    }
    return copied;
}
