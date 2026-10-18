// Runs collection scripts and assertion expressions away from the page. Every
// message it sends is treated as hostile: scripts can call postMessage too.

import { Assertion, expect } from "../../../plugin-api/assertions";

type Vars = Record<string, string>;
interface Ctx {
    vars: Vars;
    req: { method: string; url: string; headers: Record<string, string>; body: unknown; name: string };
    res?: { status: number; statusText: string; headers: Record<string, string>; body: unknown; responseTime: number };
    trust: unknown;
    logs: { level: string; text: string }[];
    tests: { name: string; passed: boolean; error?: string }[];
}
interface SendResult {
    status: number;
    status_text: string;
    headers: [string, string][];
    body: string;
}

const scope = self as unknown as { postMessage: (message: unknown) => void; onmessage: ((event: MessageEvent) => void) | null };
const pending = new Map<string, { resolve: (value: SendResult) => void; reject: (error: Error) => void }>();

// Bruno's own scripts write `expect(res).to.have.status(200)`.
Assertion.addMethod("status", function (this: Chai.AssertionStatic, expected: number) {
    const actual: unknown = this._obj;
    const status = actual && typeof actual === "object" && "status" in actual ? actual.status : actual;
    this.assert(status === expected, "expected status #{act} to be #{exp}", "expected status #{act} not to be #{exp}", expected, status);
});

function fmt(value: unknown): string {
    if (typeof value === "string") return value;
    try {
        return JSON.stringify(value);
    } catch {
        return String(value);
    }
}

function errMsg(error: unknown): string {
    return error && typeof error === "object" && "message" in error && typeof error.message === "string" ? error.message : String(error);
}

function sendRequest(req: unknown): Promise<SendResult> {
    const id = Math.random().toString(36).slice(2);
    scope.postMessage({ type: "sendRequest", id, req });
    return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
}

function makeBru(ctx: Ctx) {
    const get = (key: string) => ctx.vars[key];
    const set = (key: string, value: unknown) => {
        ctx.vars[key] = value == null ? "" : String(value);
    };
    return {
        getEnvVar: get,
        setEnvVar: set,
        getVar: get,
        setVar: set,
        getCollectionVar: get,
        getProcessEnv: (key: string) => ctx.vars["process.env." + key] || "",
        setNextRequest: () => {},
        sendRequest: async (opts: { method?: string; url: string; headers?: Record<string, unknown>; data?: unknown }) => {
            const headers = Object.entries(opts.headers || {}).map(([key, value]) => [key, String(value)]);
            const data = opts.data;
            const body =
                data == null
                    ? { kind: "none" }
                    : typeof data === "string"
                      ? { kind: "raw", content_type: null, data }
                      : { kind: "raw", content_type: "application/json", data: JSON.stringify(data) };
            const response = await sendRequest({
                method: (opts.method || "GET").toUpperCase(),
                url: opts.url,
                headers,
                body,
                timeout_ms: 0,
                skip_tls_verify: false,
                trust: ctx.trust,
            });
            let parsed: unknown = response.body;
            try {
                parsed = JSON.parse(response.body);
            } catch {
                // Not JSON, so the script gets the text.
            }
            return { status: response.status, statusText: response.status_text, headers: Object.fromEntries(response.headers), data: parsed };
        },
    };
}

function makeReq(ctx: Ctx) {
    const r = ctx.req;
    return {
        getUrl: () => r.url,
        setUrl: (url: string) => {
            r.url = url;
        },
        getMethod: () => r.method,
        setMethod: (method: string) => {
            r.method = method;
        },
        getHeaders: () => r.headers,
        getHeader: (key: string) => r.headers[key],
        setHeader: (key: string, value: string) => {
            r.headers[key] = value;
        },
        getBody: () => r.body,
        setBody: (body: unknown) => {
            r.body = body;
        },
        getName: () => r.name,
    };
}

function makeRes(r: NonNullable<Ctx["res"]>) {
    return {
        get status() {
            return r.status;
        },
        get body() {
            return r.body;
        },
        get headers() {
            return r.headers;
        },
        get responseTime() {
            return r.responseTime;
        },
        getStatus: () => r.status,
        getBody: () => r.body,
        getHeader: (key: string) => r.headers[key],
        getHeaders: () => r.headers,
        getResponseTime: () => r.responseTime,
    };
}

async function run(code: string, ctx: Ctx): Promise<void> {
    const pendingTests: Promise<unknown>[] = [];
    const test = (name: string, fn: () => unknown) => {
        try {
            const result = fn();
            if (result && typeof (result as Promise<unknown>).then === "function") {
                pendingTests.push(
                    (result as Promise<unknown>).then(
                        () => ctx.tests.push({ name, passed: true }),
                        (error: unknown) => ctx.tests.push({ name, passed: false, error: errMsg(error) }),
                    ),
                );
            } else {
                ctx.tests.push({ name, passed: true });
            }
        } catch (error) {
            ctx.tests.push({ name, passed: false, error: errMsg(error) });
        }
    };
    const log =
        (level: string) =>
        (...args: unknown[]) =>
            ctx.logs.push({ level, text: args.map(fmt).join(" ") });
    const console = { log: log("log"), info: log("info"), warn: log("warn"), error: log("error"), debug: log("debug") };
    try {
        const script = new Function("bru", "req", "res", "expect", "test", "console", "return (async () => {\n" + code + "\n})();");
        await Promise.resolve(script(makeBru(ctx), makeReq(ctx), ctx.res ? makeRes(ctx.res) : undefined, expect, test, console));
        await Promise.all(pendingTests);
    } catch (error) {
        ctx.logs.push({ level: "error", text: "script error: " + errMsg(error) });
    }
}

scope.onmessage = (event) => {
    const msg = event.data;
    if (msg.type === "sendResponse") {
        const entry = pending.get(msg.id);
        if (!entry) return;
        pending.delete(msg.id);
        if (msg.ok) entry.resolve(msg.response);
        else entry.reject(new Error(msg.error || "request failed"));
        return;
    }
    if (msg.type === "evaluate") {
        try {
            const evaluate = new Function("res", "req", "bru", "return (" + msg.expression + ");");
            scope.postMessage({ type: "evaluated", value: evaluate(msg.env.res, msg.env.req, msg.env.bru) });
        } catch (error) {
            scope.postMessage({ type: "evaluationError", error: errMsg(error) });
        }
        return;
    }
    if (msg.type !== "start") return;
    const ctx: Ctx = { vars: msg.ctx.vars, req: msg.ctx.req, res: msg.ctx.res, trust: msg.ctx.trust, logs: [], tests: [] };
    run(msg.code, ctx).then(
        () => scope.postMessage({ type: "done", vars: ctx.vars, req: ctx.req, logs: ctx.logs, tests: ctx.tests }),
        (error: unknown) =>
            scope.postMessage({
                type: "done",
                vars: ctx.vars,
                req: ctx.req,
                logs: [{ level: "error", text: "script error: " + errMsg(error) }],
                tests: ctx.tests,
            }),
    );
};
