// Turn a parsed request + its scope into a concrete HTTP call and run it via the
// Rust `bru_send` command. Resolves auth inheritance (request -> folder -> collection),
// merges headers, builds the query string and body, interpolates {{vars}}, and runs
// pre/post-request scripts + assertions through the sandbox.

import { brunoApi, type BruBodyWire, type BruSendResponse } from "../api";
import { interpolate, type Scope } from "./interpolate";
import { evaluateAssertions, evaluateExpression, runScript, type ReqView, type ResView, type ScriptLog, type TestResult } from "./sandbox";
import type { BruAuth, BruRequest, BruScope, KeyVal } from "./types";

export interface RunInput {
    request: BruRequest;
    /** collection config first, then folder scopes root→leaf */
    scopes: BruScope[];
    /** merged variable scope for interpolation (runtime overrides already folded in) */
    vars: Scope;
    runScripts?: boolean;
    /** Explicit, session-scoped approval for scripts, local endpoints and collection files. */
    trust?: boolean;
    collectionPath?: string;
}

export interface RunResult {
    response: BruSendResponse | null;
    error: string | null;
    request: { method: string; url: string; headers: [string, string][]; body: BruBodyWire };
    logs: ScriptLog[];
    tests: TestResult[];
    /** variables set by scripts during this run (apply as session runtime overrides) */
    envUpdates: Scope;
}

const enabled = (rows: KeyVal[]) => rows.filter((r) => r.enabled && r.name);

/** Walk request -> folders(leaf..root) -> collection; first non-inherit auth wins. */
export function effectiveAuth(request: BruRequest, scopes: BruScope[]): BruAuth {
    if (request.auth.mode !== "inherit") return request.auth;
    for (let i = scopes.length - 1; i >= 0; i--) {
        if (scopes[i].auth.mode !== "inherit") return scopes[i].auth;
    }
    return request.auth;
}

function applyAuth(auth: BruAuth, headers: [string, string][], query: [string, string][], scope: Scope): void {
    const I = (s: string) => interpolate(s, scope);
    switch (auth.mode) {
        case "bearer":
            if (auth.bearer.token) headers.push(["Authorization", `Bearer ${I(auth.bearer.token)}`]);
            break;
        case "basic":
            headers.push(["Authorization", `Basic ${btoa(`${I(auth.basic.username)}:${I(auth.basic.password)}`)}`]);
            break;
        case "apikey":
            if (auth.apikey.key) {
                if (auth.apikey.placement === "queryparams") query.push([I(auth.apikey.key), I(auth.apikey.value)]);
                else headers.push([I(auth.apikey.key), I(auth.apikey.value)]);
            }
            break;
        default:
            break;
    }
}

/** Parsed/interpolated body for the script's req.getBody(): object for json, string otherwise. */
function bodyForScript(request: BruRequest, scope: Scope): unknown {
    const b = request.body;
    if (b.mode === "json" || b.mode === "graphql") {
        const text = interpolate(b.text, scope);
        try {
            return JSON.parse(text);
        } catch {
            return text;
        }
    }
    if (b.mode === "text" || b.mode === "xml" || b.mode === "sparql") return interpolate(b.text, scope);
    if (b.mode === "form-urlencoded" || b.mode === "multipart-form" || b.mode === "file") {
        return Object.fromEntries(enabled(b.form).map((f) => [f.name, f.value]));
    }
    return undefined;
}

const CONTENT_TYPE: Record<string, string> = {
    json: "application/json",
    graphql: "application/json",
    xml: "application/xml",
    sparql: "application/sparql-query",
    text: "text/plain",
};

function bodyWire(request: BruRequest, reqView: ReqView, scope: Scope): BruBodyWire {
    const mode = request.body.mode;
    if (mode === "none") return { kind: "none" };
    if (mode === "file") {
        const row = enabled(request.body.form)[0];
        const path = row ? interpolate(row.value || row.name, scope) : "";
        return path ? { kind: "file", path, content_type: null } : { kind: "none" };
    }
    if (mode === "form-urlencoded") {
        return {
            kind: "form",
            fields: enabled(request.body.form).map((f) => [interpolate(f.name, scope), interpolate(f.value, scope)] as [string, string]),
        };
    }
    if (mode === "multipart-form") {
        const fileRe = /^@file\((.*)\)$/;
        return {
            kind: "multipart",
            fields: enabled(request.body.form).map((f) => {
                const m = fileRe.exec(f.value.trim());
                return m
                    ? { name: interpolate(f.name, scope), value: interpolate(m[1], scope), is_file: true }
                    : { name: interpolate(f.name, scope), value: interpolate(f.value, scope), is_file: false };
            }),
        };
    }
    // text-ish: a pre-request script may have replaced req.body
    const data = typeof reqView.body === "string" ? reqView.body : JSON.stringify(reqView.body ?? "");
    return { kind: "raw", content_type: CONTENT_TYPE[mode] ?? "text/plain", data };
}

function enc(v: string, encode: boolean): string {
    return encode ? encodeURIComponent(v) : v;
}

function escapeRegExp(v: string): string {
    return v.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function applyPathParams(url: string, params: [string, string][], encode: boolean): string {
    let out = url;
    for (const [k, v] of params) {
        if (!k) continue;
        const val = enc(v, encode);
        const key = escapeRegExp(k);
        out = out.replace(new RegExp(`:${key}(?=/|$|[?#])`, "g"), val);
        out = out.replace(new RegExp(`\\{${key}\\}`, "g"), val);
    }
    return out;
}

function applyQuery(url: string, query: [string, string][], encode: boolean): string {
    if (!query.length) return url;
    const hashAt = url.indexOf("#");
    const beforeHash = hashAt === -1 ? url : url.slice(0, hashAt);
    const hash = hashAt === -1 ? "" : url.slice(hashAt);
    const qs = query.map(([k, v]) => `${enc(k, encode)}=${enc(v, encode)}`).join("&");
    if (!qs) return url;
    const sep = beforeHash.includes("?") ? (beforeHash.endsWith("?") || beforeHash.endsWith("&") ? "" : "&") : "?";
    return `${beforeHash}${sep}${qs}${hash}`;
}

function exprValue(v: unknown): string {
    if (v == null) return "";
    if (typeof v === "string") return v;
    if (typeof v === "number" || typeof v === "boolean") return String(v);
    try {
        return JSON.stringify(v);
    } catch {
        return String(v);
    }
}

async function applyResponseVars(
    rows: KeyVal[],
    env: { res: ResView; req: ReqView; bru: Record<string, unknown> },
    scope: Scope,
    logs: ScriptLog[],
): Promise<void> {
    for (const row of enabled(rows)) {
        if (!row.value.trim()) {
            scope[row.name] = "";
            continue;
        }
        try {
            const value = exprValue(await evaluateExpression(row.value, env));
            scope[row.name] = value;
            env.bru[row.name] = value;
        } catch (e) {
            const msg = e instanceof Error ? e.message : String(e);
            logs.push({ level: "error", text: `post-response var ${row.name}: ${msg}` });
        }
    }
}

export async function runRequest(input: RunInput): Promise<RunResult> {
    const { request, scopes } = input;
    const runScripts = input.runScripts !== false;
    // Single mutable scope the scripts read/write via bru.getEnvVar/setEnvVar.
    // Seeded with the full resolved scope so getEnvVar("token") sees env/secret
    // values, not just prior overrides.
    const scope: Scope = { ...input.vars };

    const logs: ScriptLog[] = [];
    const tests: TestResult[] = [];
    const trust = {
        allow_private_network: input.trust === true,
        allow_file_read: input.trust === true,
        allow_insecure_tls: input.trust === true,
        file_root: input.collectionPath ?? null,
    };

    // Pre-script view of the request (interpolated, no auth applied yet).
    const preHeaders: Record<string, string> = {};
    for (const s of scopes) for (const h of enabled(s.headers)) preHeaders[interpolate(h.name, scope)] = interpolate(h.value, scope);
    for (const h of enabled(request.headers)) preHeaders[interpolate(h.name, scope)] = interpolate(h.value, scope);

    const reqView: ReqView = {
        method: request.method.toUpperCase(),
        url: interpolate(request.url, scope),
        headers: preHeaders,
        body: bodyForScript(request, scope),
        name: request.meta.name,
    };

    if (runScripts) {
        for (const s of scopes) await runScript(s.scripts.pre, { vars: scope, req: reqView, logs, tests, trust });
        await runScript(request.scripts.pre, { vars: scope, req: reqView, logs, tests, trust });
    }

    // Final build using the (possibly script-mutated) reqView + post-script scope.
    const finalScope = scope;
    const headers: [string, string][] = Object.entries(reqView.headers).map(([k, v]) => [k, interpolate(v, finalScope)]);
    const encodeUrl = request.settings.encodeUrl !== false;
    const pathParams: [string, string][] = enabled(request.params.path).map((p) => [
        interpolate(p.name, finalScope),
        interpolate(p.value, finalScope),
    ]);
    const query: [string, string][] = enabled(request.params.query).map((p) => [interpolate(p.name, finalScope), interpolate(p.value, finalScope)]);
    const auth = effectiveAuth(request, scopes);
    applyAuth(auth, headers, query, finalScope);
    const url = applyQuery(applyPathParams(interpolate(reqView.url, finalScope), pathParams, encodeUrl), query, encodeUrl);
    const wire = {
        method: reqView.method,
        url,
        headers,
        body: bodyWire(request, reqView, finalScope),
        timeout_ms: request.settings.timeout && request.settings.timeout > 0 ? request.settings.timeout : 0,
        skip_tls_verify: false,
        trust,
    };
    const meta = { method: wire.method, url: wire.url, headers: wire.headers, body: wire.body };
    const envUpdates = (): Scope => {
        const out: Scope = {};
        for (const k of Object.keys(scope)) if (scope[k] !== input.vars[k]) out[k] = scope[k];
        return out;
    };

    let response: BruSendResponse | null = null;
    try {
        response = await brunoApi.send(wire);
    } catch (e) {
        const err = e as { message?: string };
        return { response: null, error: err?.message ?? String(e), request: meta, logs, tests, envUpdates: envUpdates() };
    }

    if (runScripts) {
        let parsed: unknown = response.body;
        try {
            parsed = JSON.parse(response.body);
        } catch {
            /* keep raw */
        }
        const resView: ResView = {
            status: response.status,
            statusText: response.status_text,
            headers: Object.fromEntries(response.headers),
            body: parsed,
            responseTime: response.duration_ms,
        };
        for (const s of scopes) await runScript(s.scripts.post, { vars: scope, req: reqView, res: resView, logs, tests, trust });
        await runScript(request.scripts.post, { vars: scope, req: reqView, res: resView, logs, tests, trust });
        const varEnv = { res: resView, req: reqView, bru: { ...scope } };
        for (const s of scopes) await applyResponseVars(s.vars.res, varEnv, scope, logs);
        await applyResponseVars(request.vars.res, { ...varEnv, bru: { ...scope } }, scope, logs);
        if (request.assertions.length) {
            await evaluateAssertions(request.assertions, { res: resView, req: reqView, bru: { ...finalScope } }, tests);
        }
    }

    return { response, error: null, request: meta, logs, tests, envUpdates: envUpdates() };
}
