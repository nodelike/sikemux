// Load a Bruno collection (or a workspace of collections) from disk.
//
// A Bruno *collection* is a directory with a `bruno.json` / `collection.bru`;
// its environments live in `<collection>/environments/*.bru`. A *workspace* may
// hold several collections (e.g. `collections/api-gateway`, `collections/surepass`).
// We support both: open a single collection, or a workspace folder above many.
// Environments are gathered from every collection (labelled by collection name
// when nested), and a nested `collection.bru` participates in scope inheritance
// just like a `folder.bru`, so per-collection auth / token-refresh scripts apply.

import { files as fsapi, type DirEntry } from "../../../plugin-api/host";
import { parseEnv, parseRequest, parseScope } from "./parse";
import type { BruCollection, BruEnv, BruTreeNode } from "./types";

const BRU = ".bru";
const ENV_DIR = "environments";
const SEQ_LAST = Number.MAX_SAFE_INTEGER;
const IGNORE_DIRS = new Set([".git", "node_modules", ENV_DIR]);

// A collection is a directory tree, so reading every sibling at once means the
// whole tree is in flight at once. Enough pending reads and the browser spends
// longer tracking the promises than answering them, so only this many run.
const READ_CONCURRENCY = 16;

async function mapBounded<In, Out>(items: readonly In[], run: (item: In) => Promise<Out>): Promise<Out[]> {
    const results = new Array<Out>(items.length);
    let next = 0;
    const workers = Array.from({ length: Math.min(READ_CONCURRENCY, items.length) }, async () => {
        for (let at = next++; at < items.length; at = next++) results[at] = await run(items[at]);
    });
    await Promise.all(workers);
    return results;
}

function baseName(p: string): string {
    const i = Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\"));
    return i < 0 ? p : p.slice(i + 1);
}

function stem(name: string): string {
    return name.endsWith(BRU) ? name.slice(0, -BRU.length) : name;
}

function isCollectionRoot(entries: DirEntry[]): boolean {
    return entries.some((e) => !e.is_dir && (e.name === "bruno.json" || e.name === "collection.bru"));
}

/**
 * Build the folder/request tree. `environments/` dirs are skipped (handled
 * separately). `collPath` tracks the nearest enclosing collection root so each
 * request can be scoped to its collection's environments.
 */
async function buildTree(entries: DirEntry[], collPath: string): Promise<BruTreeNode[]> {
    // One directory listing or file read at a time meant a collection of a few
    // hundred requests was that many round trips end to end. Siblings do not
    // depend on each other, so they are read together.
    const nodes = await mapBounded(entries, (entry) => buildNode(entry, collPath));
    return nodes.filter((node): node is BruTreeNode => node !== null).sort((a, b) => a.seq - b.seq || a.name.localeCompare(b.name));
}

async function buildNode(entry: DirEntry, collPath: string): Promise<BruTreeNode | null> {
    if (entry.is_dir) {
        if (IGNORE_DIRS.has(entry.name)) return null;
        const children = await fsapi.readDir(entry.path);
        // A folder's scope comes from folder.bru, or a nested collection.bru.
        const scopeFile = children.find((c) => !c.is_dir && (c.name === "folder.bru" || c.name === "collection.bru"));
        const [scope, childNodes] = await Promise.all([
            scopeFile ? fsapi.readFile(scopeFile.path).then((text) => parseScope(text, entry.name)) : Promise.resolve(null),
            buildTree(children, isCollectionRoot(children) ? entry.path : collPath),
        ]);
        // Skip empty structural dirs (e.g. a bare `collections/` wrapper with nothing useful).
        if (childNodes.length === 0 && !scope) return null;
        return {
            type: "folder",
            name: scope?.meta.name || entry.name,
            path: entry.path,
            seq: scope?.meta.seq ?? SEQ_LAST,
            scope,
            children: childNodes,
        };
    }
    if (!entry.name.endsWith(BRU) || entry.name === "collection.bru" || entry.name === "folder.bru") return null;
    const request = parseRequest(await fsapi.readFile(entry.path));
    return {
        type: "request",
        name: request.meta.name || stem(entry.name),
        path: entry.path,
        seq: request.meta.seq ?? SEQ_LAST,
        method: request.method,
        collectionPath: collPath,
        request,
    };
}

/**
 * Recursively gather environments from every `environments/` dir, tagging each
 * with the collection root it belongs to (so the UI can scope the env dropdown
 * to the open request's collection).
 */
async function collectEnvs(dirPath: string, entries: DirEntry[], collPath: string): Promise<BruEnv[]> {
    const here = isCollectionRoot(entries) ? dirPath : collPath;
    const groups = await mapBounded(entries, async (e): Promise<BruEnv[]> => {
        if (!e.is_dir) return [];
        if (e.name === ENV_DIR) {
            const files = (await fsapi.readDir(e.path)).filter((f) => !f.is_dir && f.name.endsWith(BRU));
            return mapBounded(files, async (f) => parseEnv(await fsapi.readFile(f.path), stem(f.name), here, baseName(here || dirPath)));
        }
        if (IGNORE_DIRS.has(e.name)) return [];
        return collectEnvs(e.path, await fsapi.readDir(e.path), here);
    });
    return groups.flat();
}

export async function loadCollection(rootPath: string): Promise<BruCollection> {
    const entries = await fsapi.readDir(rootPath);

    const collectionBru = entries.find((e) => !e.is_dir && e.name === "collection.bru");
    const rootColl = isCollectionRoot(entries) ? rootPath : "";
    const [config, envs, tree] = await Promise.all([
        collectionBru ? fsapi.readFile(collectionBru.path).then((text) => parseScope(text, baseName(rootPath))) : Promise.resolve(null),
        collectEnvs(rootPath, entries, rootColl),
        buildTree(entries, rootColl),
    ]);
    envs.sort((a, b) => a.collectionName.localeCompare(b.collectionName) || a.name.localeCompare(b.name));
    return { rootPath, name: config?.meta.name || baseName(rootPath), config, envs, tree };
}
