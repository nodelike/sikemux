// Helpers tying a request to its place in the collection: locating a request by
// path (collecting the folder scopes above it) and assembling the variable scope
// used for {{interpolation}}.

import { mergeScope, type Scope } from "./interpolate";
import type { BruCollection, BruEnv, BruRequest, BruScope, BruTreeNode, KeyVal } from "./types";

export interface LocatedRequest {
    request: BruRequest;
    /** ancestor folder scopes, root-first */
    folderScopes: BruScope[];
    /** collection root the request belongs to */
    collectionPath: string;
}

export function findRequest(nodes: BruTreeNode[], path: string, acc: BruScope[] = []): LocatedRequest | null {
    for (const n of nodes) {
        if (n.type === "request") {
            if (n.path === path) return { request: n.request, folderScopes: acc, collectionPath: n.collectionPath };
        } else {
            const found = findRequest(n.children, path, n.scope ? [...acc, n.scope] : acc);
            if (found) return found;
        }
    }
    return null;
}

function varsOf(scope: BruScope | null): Scope {
    const m: Scope = {};
    if (scope) for (const v of scope.vars.req) if (v.enabled && v.name) m[v.name] = v.value;
    return m;
}

function varsOfRows(rows: KeyVal[]): Scope {
    const m: Scope = {};
    for (const v of rows) if (v.enabled && v.name) m[v.name] = v.value;
    return m;
}

function envVars(env: BruEnv | undefined): Scope {
    const m: Scope = {};
    if (env) for (const v of env.vars) if (v.enabled && v.name) m[v.name] = v.value;
    return m;
}

/** Variable scope, precedence high→low: secrets, environment, folders(leaf→root), collection. */
export function buildScope(opts: {
    collection: BruCollection;
    env: BruEnv | undefined;
    secretVars: Record<string, string>;
    folderScopes: BruScope[];
}): Scope {
    const folderLayers = [...opts.folderScopes].reverse().map(varsOf);
    return mergeScope(opts.secretVars, envVars(opts.env), ...folderLayers, varsOf(opts.collection.config));
}

export function requestVars(request: BruRequest | null | undefined): Scope {
    return request ? varsOfRows(request.vars.req) : {};
}

export function selectedEnvOf(collection: BruCollection, id: string | null): BruEnv | undefined {
    return id ? collection.envs.find((e) => e.id === id) : undefined;
}
