// Helpers tying a request to its place in the collection: finding its tree row by path, and
// assembling the variable scope used for {{interpolation}}.

import { mergeScope, type Scope } from "./interpolate";
import type { BruEnv, BruRequest, BruScope, BruTreeNode, KeyVal } from "./types";

export type BruTreeRequest = Extract<BruTreeNode, { type: "request" }>;

export function findRequest(nodes: BruTreeNode[], path: string): BruTreeRequest | null {
    for (const node of nodes) {
        if (node.type === "request") {
            if (node.path === path) return node;
        } else {
            const found = findRequest(node.children, path);
            if (found) return found;
        }
    }
    return null;
}

function varsOfRows(rows: KeyVal[]): Scope {
    const m: Scope = {};
    for (const v of rows) if (v.enabled && v.name) m[v.name] = v.value;
    return m;
}

/** Variable scope, precedence high→low: secrets, environment, then the scopes from the request's folder out to the collection. */
export function buildScope(opts: { env: BruEnv | undefined; secretVars: Record<string, string>; scopes: BruScope[] }): Scope {
    const layers = [...opts.scopes].reverse().map((scope) => varsOfRows(scope.vars.req));
    return mergeScope(opts.secretVars, varsOfRows(opts.env?.vars ?? []), ...layers);
}

export function requestVars(request: BruRequest | null | undefined): Scope {
    return request ? varsOfRows(request.vars.req) : {};
}

export function selectedEnvOf(envs: BruEnv[], id: string | null): BruEnv | undefined {
    return id ? envs.find((e) => e.id === id) : undefined;
}
