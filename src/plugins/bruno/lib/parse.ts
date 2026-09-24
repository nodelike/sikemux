// Parser for the Bruno (.bru) block DSL.
//
// A .bru file is a sequence of top-level blocks: `name { ... }` (dict / verbatim
// text) or `name [ ... ]` (list). We tokenize generically (brace-aware, and
// skipping `'''…'''` heredocs so braces inside example bodies don't confuse the
// counter), then fold known blocks into the typed model and keep the rest raw.

import type { AuthMode, BodyMode, BruEnv, BruRequest, BruScope, HttpMethod, KeyVal, RawBlock } from "./types";
import { emptyAuth, emptyBody, HTTP_METHODS } from "./types";

export function lexBlocks(src: string): RawBlock[] {
    const blocks: RawBlock[] = [];
    const n = src.length;
    let i = 0;
    while (i < n) {
        while (i < n && /\s/.test(src[i])) i++;
        if (i >= n) break;
        const nameStart = i;
        while (i < n && /[A-Za-z0-9:_-]/.test(src[i])) i++;
        const name = src.slice(nameStart, i);
        if (!name) {
            i++;
            continue;
        }
        while (i < n && (src[i] === " " || src[i] === "\t")) i++;
        const open = src[i];
        if (open !== "{" && open !== "[") {
            while (i < n && src[i] !== "\n") i++; // not a block — skip the line
            continue;
        }
        const close = open === "{" ? "}" : "]";
        i++;
        const innerStart = i;
        let depth = 1;
        while (i < n) {
            const c = src[i];
            if (open === "{" && c === "'" && src[i + 1] === "'" && src[i + 2] === "'") {
                i += 3;
                while (i < n && !(src[i] === "'" && src[i + 1] === "'" && src[i + 2] === "'")) i++;
                i += 3;
                continue;
            }
            if (c === open) {
                depth++;
                i++;
                continue;
            }
            if (c === close) {
                depth--;
                if (depth === 0) break;
                i++;
                continue;
            }
            i++;
        }
        const inner = src.slice(innerStart, i);
        i++; // consume close delimiter
        blocks.push({ name, delim: open as "{" | "[", raw: inner });
    }
    return blocks;
}

/** Strip a uniform leading indent from a verbatim block's inner text. */
export function dedent(raw: string): string {
    const lines = raw.split("\n");
    while (lines.length && lines[0].trim() === "") lines.shift();
    while (lines.length && lines[lines.length - 1].trim() === "") lines.pop();
    if (!lines.length) return "";
    let min = Infinity;
    for (const l of lines) {
        if (l.trim() === "") continue;
        const m = (l.match(/^[ \t]*/) as RegExpMatchArray)[0].length;
        if (m < min) min = m;
    }
    if (!isFinite(min)) min = 0;
    return lines.map((l) => (l.trim() === "" ? "" : l.slice(min))).join("\n");
}

export function parseKeyVals(raw: string): KeyVal[] {
    const out: KeyVal[] = [];
    for (const line of raw.split("\n")) {
        const t = line.trim();
        if (!t) continue;
        let enabled = true;
        let body = t;
        if (body.startsWith("~")) {
            enabled = false;
            body = body.slice(1).trimStart();
        }
        const ci = body.indexOf(":");
        if (ci === -1) {
            out.push({ name: body, value: "", enabled });
            continue;
        }
        const name = body.slice(0, ci).trim();
        let value = body.slice(ci + 1);
        if (value.startsWith(" ")) value = value.slice(1);
        out.push({ name, value, enabled });
    }
    return out;
}

function dictMap(raw: string): Map<string, string> {
    const m = new Map<string, string>();
    for (const kv of parseKeyVals(raw)) if (kv.enabled && !m.has(kv.name)) m.set(kv.name, kv.value);
    return m;
}

function parseList(raw: string): string[] {
    return raw
        .split(/[,\n]/)
        .map((s) => s.trim())
        .filter(Boolean);
}

function normBodyRef(v: string): BodyMode | null {
    switch (v) {
        case "json":
        case "text":
        case "xml":
        case "sparql":
        case "graphql":
        case "file":
        case "none":
            return v;
        case "formUrlEncoded":
        case "form-urlencoded":
            return "form-urlencoded";
        case "multipartForm":
        case "multipart-form":
            return "multipart-form";
        default:
            return null;
    }
}

const AUTH_MODES = new Set<string>(["none", "inherit", "bearer", "basic", "apikey", "awsv4", "digest", "wsse", "oauth2"]);
function normAuthMode(v: string): AuthMode | null {
    return AUTH_MODES.has(v) ? (v as AuthMode) : null;
}

function applyAuthDetail(auth: BruRequest["auth"], name: string, raw: string): void {
    const m = dictMap(raw);
    if (name === "auth:bearer") auth.bearer.token = m.get("token") ?? "";
    else if (name === "auth:basic") {
        auth.basic.username = m.get("username") ?? "";
        auth.basic.password = m.get("password") ?? "";
    } else if (name === "auth:apikey") {
        auth.apikey.key = m.get("key") ?? "";
        auth.apikey.value = m.get("value") ?? "";
        const place = m.get("placement") ?? m.get("addTo") ?? "header";
        auth.apikey.placement = place.toLowerCase().includes("query") ? "queryparams" : "header";
    }
}

export function parseRequest(src: string): BruRequest {
    const req: BruRequest = {
        meta: { name: "", type: "http" },
        method: "get",
        url: "",
        params: { query: [], path: [] },
        headers: [],
        body: emptyBody(),
        auth: emptyAuth("inherit"),
        vars: { req: [], res: [] },
        scripts: { pre: "", post: "" },
        assertions: [],
        tests: "",
        docs: "",
        settings: {},
        extra: [],
    };
    let bodyModeFromMethod: BodyMode | null = null;
    let authModeFromMethod: AuthMode | null = null;
    const bodyBlocks: { mode: BodyMode; block: RawBlock }[] = [];

    for (const b of lexBlocks(src)) {
        const { name, raw } = b;
        if (name === "meta") {
            const m = dictMap(raw);
            req.meta.name = m.get("name") ?? "";
            req.meta.type = m.get("type") ?? "http";
            const seq = m.get("seq");
            if (seq != null && seq !== "") req.meta.seq = Number(seq);
        } else if (HTTP_METHODS.includes(name as HttpMethod)) {
            req.method = name as HttpMethod;
            const m = dictMap(raw);
            req.url = m.get("url") ?? "";
            const bref = m.get("body");
            if (bref != null) bodyModeFromMethod = normBodyRef(bref);
            const aref = m.get("auth");
            if (aref != null) authModeFromMethod = normAuthMode(aref);
        } else if (name === "headers") {
            req.headers = parseKeyVals(raw);
        } else if (name === "params:query") {
            req.params.query = parseKeyVals(raw);
        } else if (name === "params:path") {
            req.params.path = parseKeyVals(raw);
        } else if (name.startsWith("body:") && normBodyRef(name.slice("body:".length)) != null) {
            bodyBlocks.push({ mode: name.slice("body:".length) as BodyMode, block: b });
        } else if (name === "auth") {
            const mode = normAuthMode(dictMap(raw).get("mode") ?? "");
            if (mode) authModeFromMethod = authModeFromMethod ?? mode;
        } else if (name === "auth:bearer" || name === "auth:basic" || name === "auth:apikey") {
            applyAuthDetail(req.auth, name, raw);
        } else if (name === "vars:pre-request" || name === "vars") {
            req.vars.req = parseKeyVals(raw);
        } else if (name === "vars:post-response") {
            req.vars.res = parseKeyVals(raw);
        } else if (name === "assert") {
            req.assertions = parseKeyVals(raw);
        } else if (name === "script:pre-request") {
            req.scripts.pre = dedent(raw);
        } else if (name === "script:post-response") {
            req.scripts.post = dedent(raw);
        } else if (name === "tests") {
            req.tests = dedent(raw);
        } else if (name === "docs") {
            req.docs = dedent(raw);
        } else if (name === "settings") {
            const m = dictMap(raw);
            if (m.has("encodeUrl")) req.settings.encodeUrl = m.get("encodeUrl") === "true";
            if (m.has("timeout")) req.settings.timeout = Number(m.get("timeout"));
        } else {
            req.extra.push(b); // example, and any block we don't model — kept verbatim
        }
    }

    // Body mode is driven by the method block's `body:` ref (Bruno's source of
    // truth for the *active* body). Content blocks whose type matches the active
    // mode are folded into the typed body; any orphaned content (e.g. leftover
    // body:json while mode is none) is preserved verbatim so it round-trips.
    req.body.mode = bodyModeFromMethod ?? bodyBlocks[0]?.mode ?? "none";
    let consumed = false;
    for (const { mode, block } of bodyBlocks) {
        if (!consumed && mode === req.body.mode) {
            consumed = true;
            if (mode === "form-urlencoded" || mode === "multipart-form" || mode === "file") req.body.form = parseKeyVals(block.raw);
            else req.body.text = dedent(block.raw);
        } else {
            req.extra.push(block);
        }
    }
    req.auth.mode = authModeFromMethod ?? "inherit";
    return req;
}

export function parseScope(src: string, name = ""): BruScope {
    const scope: BruScope = {
        meta: { name, type: "" },
        auth: emptyAuth("inherit"),
        headers: [],
        vars: { req: [], res: [] },
        scripts: { pre: "", post: "" },
        extra: [],
    };
    for (const b of lexBlocks(src)) {
        if (b.name === "meta") {
            const m = dictMap(b.raw);
            scope.meta.name = m.get("name") ?? name;
            if (m.has("type")) scope.meta.type = m.get("type") ?? "";
            const seq = m.get("seq");
            if (seq != null && seq !== "") scope.meta.seq = Number(seq);
        } else if (b.name === "auth") {
            const mode = normAuthMode(dictMap(b.raw).get("mode") ?? "");
            if (mode) scope.auth.mode = mode;
        } else if (b.name === "auth:bearer" || b.name === "auth:basic" || b.name === "auth:apikey") {
            applyAuthDetail(scope.auth, b.name, b.raw);
        } else if (b.name === "headers") {
            scope.headers = parseKeyVals(b.raw);
        } else if (b.name === "vars" || b.name === "vars:pre-request") {
            scope.vars.req = parseKeyVals(b.raw);
        } else if (b.name === "vars:post-response") {
            scope.vars.res = parseKeyVals(b.raw);
        } else if (b.name === "script:pre-request") {
            scope.scripts.pre = dedent(b.raw);
        } else if (b.name === "script:post-response") {
            scope.scripts.post = dedent(b.raw);
        } else {
            scope.extra.push(b);
        }
    }
    return scope;
}

export function parseEnv(src: string, name: string, collectionPath = "", collectionName = ""): BruEnv {
    const env: BruEnv = { id: `${collectionPath}::${name}`, name, collectionPath, collectionName, vars: [], secretNames: [] };
    for (const b of lexBlocks(src)) {
        if (b.name === "vars") env.vars = parseKeyVals(b.raw);
        else if (b.name === "vars:secret") env.secretNames = parseList(b.raw);
    }
    return env;
}
