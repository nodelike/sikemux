// Serializer: typed model -> .bru text. Inverse of parse.ts, emitting blocks in
// Bruno's canonical order and re-indenting verbatim text. Round-trips with
// parse.ts at the model level (parse(serialize(parse(x))) deep-equals parse(x)).

import type { BodyMode, BruEnv, BruRequest, BruScope, KeyVal, RawBlock } from "./types";

function indent(text: string, n = 2): string {
    if (text === "") return "";
    const pad = " ".repeat(n);
    return text
        .split("\n")
        .map((l) => (l === "" ? "" : pad + l))
        .join("\n");
}

function kvLine(kv: KeyVal): string {
    return `  ${kv.enabled ? "" : "~"}${kv.name}: ${kv.value}`;
}

function dictBlock(name: string, rows: KeyVal[]): string | null {
    if (rows.length === 0) return null;
    return `${name} {\n${rows.map(kvLine).join("\n")}\n}`;
}

function textBlock(name: string, text: string): string | null {
    if (!text) return null;
    return `${name} {\n${indent(text)}\n}`;
}

function rawBlock(b: RawBlock): string {
    const close = b.delim === "{" ? "}" : "]";
    return `${b.name} ${b.delim}${b.raw}${close}`;
}

function bodyRef(mode: BodyMode): string {
    if (mode === "form-urlencoded") return "formUrlEncoded";
    if (mode === "multipart-form") return "multipartForm";
    return mode;
}

function metaBlock(meta: { name: string; type?: string; seq?: number }): string {
    const lines = [`  name: ${meta.name}`];
    if (meta.type) lines.push(`  type: ${meta.type}`);
    if (meta.seq != null) lines.push(`  seq: ${meta.seq}`);
    return `meta {\n${lines.join("\n")}\n}`;
}

function authDetailBlock(auth: BruRequest["auth"]): string | null {
    if (auth.mode === "bearer") return `auth:bearer {\n  token: ${auth.bearer.token}\n}`;
    if (auth.mode === "basic") return `auth:basic {\n  username: ${auth.basic.username}\n  password: ${auth.basic.password}\n}`;
    if (auth.mode === "apikey") {
        const place = auth.apikey.placement === "queryparams" ? "queryparams" : "header";
        return `auth:apikey {\n  key: ${auth.apikey.key}\n  value: ${auth.apikey.value}\n  placement: ${place}\n}`;
    }
    return null;
}

function bodyBlock(body: BruRequest["body"]): string | null {
    switch (body.mode) {
        case "none":
            return null;
        case "json":
        case "text":
        case "xml":
        case "sparql":
        case "graphql":
            return textBlock(`body:${body.mode}`, body.text);
        case "form-urlencoded":
            return dictBlock("body:form-urlencoded", body.form);
        case "multipart-form":
            return dictBlock("body:multipart-form", body.form);
        case "file":
            return dictBlock("body:file", body.form);
    }
}

export function serializeRequest(req: BruRequest): string {
    const parts: (string | null)[] = [];
    parts.push(metaBlock(req.meta));
    parts.push(`${req.method} {\n  url: ${req.url}\n  body: ${bodyRef(req.body.mode)}\n  auth: ${req.auth.mode}\n}`);
    parts.push(dictBlock("params:query", req.params.query));
    parts.push(dictBlock("params:path", req.params.path));
    parts.push(dictBlock("headers", req.headers));
    parts.push(authDetailBlock(req.auth));
    parts.push(bodyBlock(req.body));
    parts.push(dictBlock("vars:pre-request", req.vars.req));
    parts.push(dictBlock("vars:post-response", req.vars.res));
    parts.push(dictBlock("assert", req.assertions));
    parts.push(textBlock("script:pre-request", req.scripts.pre));
    parts.push(textBlock("script:post-response", req.scripts.post));
    parts.push(textBlock("tests", req.tests));
    parts.push(textBlock("docs", req.docs));

    const settingsLines: string[] = [];
    if (req.settings.encodeUrl != null) settingsLines.push(`  encodeUrl: ${req.settings.encodeUrl}`);
    if (req.settings.timeout != null) settingsLines.push(`  timeout: ${req.settings.timeout}`);
    if (settingsLines.length) parts.push(`settings {\n${settingsLines.join("\n")}\n}`);

    for (const b of req.extra) parts.push(rawBlock(b));

    return parts.filter((p): p is string => p != null).join("\n\n") + "\n";
}

export function serializeScope(scope: BruScope): string {
    const parts: (string | null)[] = [];
    if (scope.meta.name || scope.meta.seq != null) parts.push(metaBlock(scope.meta));
    if (scope.auth.mode !== "inherit") parts.push(`auth {\n  mode: ${scope.auth.mode}\n}`);
    parts.push(authDetailBlock(scope.auth));
    parts.push(dictBlock("headers", scope.headers));
    parts.push(dictBlock("vars", scope.vars.req));
    parts.push(dictBlock("vars:post-response", scope.vars.res));
    parts.push(textBlock("script:pre-request", scope.scripts.pre));
    parts.push(textBlock("script:post-response", scope.scripts.post));
    for (const b of scope.extra) parts.push(rawBlock(b));
    return parts.filter((p): p is string => p != null).join("\n\n") + "\n";
}

export function serializeEnv(env: BruEnv): string {
    const parts: string[] = [];
    parts.push(`vars {\n${env.vars.map(kvLine).join("\n")}\n}`);
    if (env.secretNames.length) parts.push(`vars:secret [\n${env.secretNames.map((s) => `  ${s}`).join(",\n")}\n]`);
    return parts.join("\n\n") + "\n";
}
