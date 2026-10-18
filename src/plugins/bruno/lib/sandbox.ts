// Pre/post-request scripts and assertion expressions execute only inside
// disposable workers. The parent owns all privileged bridges and re-applies
// the user-approved trust policy to every worker-originated network request.

import { brunoApi, type BruSendRequest } from "../api";
import type { Scope } from "./interpolate";

export interface ScriptLog {
    level: string;
    text: string;
}
export interface TestResult {
    name: string;
    passed: boolean;
    error?: string;
}

export interface ReqView {
    method: string;
    url: string;
    headers: Record<string, string>;
    body: unknown;
    name: string;
}
export interface ResView {
    status: number;
    statusText: string;
    headers: Record<string, string>;
    body: unknown;
    responseTime: number;
}

export interface SandboxCtx {
    vars: Scope;
    req: ReqView;
    res?: ResView;
    logs: ScriptLog[];
    tests: TestResult[];
    trust: BruSendRequest["trust"];
}

const SCRIPT_TIMEOUT_MS = 15_000;

function fmt(v: unknown): string {
    if (typeof v === "string") return v;
    try {
        return JSON.stringify(v);
    } catch {
        return String(v);
    }
}

function errMsg(e: unknown): string {
    return e instanceof Error ? e.message : String(e);
}

function includes(actual: unknown, v: unknown): boolean {
    if (typeof actual === "string") return actual.includes(String(v));
    if (Array.isArray(actual)) return actual.includes(v);
    if (actual && typeof actual === "object") return String(v) in (actual as object);
    return false;
}

function isEmpty(v: unknown): boolean {
    if (v == null) return true;
    if (typeof v === "string" || Array.isArray(v)) return v.length === 0;
    if (typeof v === "object") return Object.keys(v as object).length === 0;
    return false;
}

function replaceScope(target: Scope, next: Scope): void {
    for (const key of Object.keys(target)) delete target[key];
    Object.assign(target, next);
}

function replaceReq(target: ReqView, next: ReqView): void {
    target.method = next.method;
    target.url = next.url;
    target.headers = next.headers;
    target.body = next.body;
    target.name = next.name;
}

interface WorkerDone {
    type: "done";
    vars: Scope;
    req: ReqView;
    logs: ScriptLog[];
    tests: TestResult[];
}

interface WorkerSendRequest {
    type: "sendRequest";
    id: string;
    req: BruSendRequest;
}

type WorkerMessage = WorkerDone | WorkerSendRequest;

/** Worker messages are attacker-controlled: collection code can call
 * `self.postMessage` directly. Never honor trust flags supplied by the worker. */
export function constrainWorkerRequest(request: BruSendRequest, trust: BruSendRequest["trust"]): BruSendRequest {
    return { ...request, trust: { ...trust } };
}

function isWorkerAvailable(): boolean {
    return typeof Worker !== "undefined";
}

function startWorker(): Worker {
    return new Worker(new URL("./sandbox.worker.ts", import.meta.url), { type: "module" });
}

export async function runScript(code: string, ctx: SandboxCtx): Promise<void> {
    if (!code.trim()) return;
    if (!isWorkerAvailable()) {
        ctx.logs.push({ level: "error", text: "script error: isolated script worker unavailable" });
        return;
    }

    const worker = startWorker();
    let settled = false;

    await new Promise<void>((resolve) => {
        const finish = (err?: string) => {
            if (settled) return;
            settled = true;
            clearTimeout(timeout);
            worker.terminate();
            if (err) ctx.logs.push({ level: "error", text: `script error: ${err}` });
            resolve();
        };

        const timeout = setTimeout(() => finish("script timed out"), SCRIPT_TIMEOUT_MS);

        worker.onmessage = (event: MessageEvent<WorkerMessage>) => {
            const msg = event.data;
            if (msg.type === "sendRequest") {
                const request = constrainWorkerRequest(msg.req, ctx.trust);
                void brunoApi.send(request).then(
                    (response) => worker.postMessage({ type: "sendResponse", id: msg.id, ok: true, response }),
                    (e: unknown) => worker.postMessage({ type: "sendResponse", id: msg.id, ok: false, error: errMsg(e) }),
                );
                return;
            }
            replaceScope(ctx.vars, msg.vars);
            replaceReq(ctx.req, msg.req);
            ctx.logs.push(...msg.logs);
            ctx.tests.push(...msg.tests);
            finish();
        };
        worker.onerror = (event) => finish(event.message || "worker error");
        worker.postMessage({
            type: "start",
            code,
            ctx: {
                vars: { ...ctx.vars },
                req: { ...ctx.req, headers: { ...ctx.req.headers } },
                res: ctx.res ? { ...ctx.res, headers: { ...ctx.res.headers } } : undefined,
                trust: ctx.trust,
            },
        });
    });
}

const ASSERT_OPS = new Set([
    "eq",
    "neq",
    "gt",
    "gte",
    "lt",
    "lte",
    "in",
    "notIn",
    "contains",
    "notContains",
    "length",
    "matches",
    "notMatches",
    "startsWith",
    "endsWith",
    "between",
    "isEmpty",
    "notEmpty",
    "isNull",
    "isUndefined",
    "isDefined",
    "isTruthy",
    "isFalsy",
    "isJson",
    "isNumber",
    "isString",
    "isBoolean",
    "isArray",
]);

function coerce(s: string): unknown {
    const t = s.trim();
    if (t === "") return "";
    try {
        return JSON.parse(t);
    } catch {
        return t.replace(/^['"]|['"]$/g, "");
    }
}

export async function evaluateExpression(expr: string, env: { res?: ResView; req: ReqView; bru: Record<string, unknown> }): Promise<unknown> {
    if (!isWorkerAvailable()) throw new Error("isolated expression worker unavailable");
    const worker = startWorker();
    return new Promise((resolve, reject) => {
        let settled = false;
        const finish = (error?: string, value?: unknown) => {
            if (settled) return;
            settled = true;
            clearTimeout(timeout);
            worker.terminate();
            if (error) reject(new Error(error));
            else resolve(value);
        };
        const timeout = setTimeout(() => finish("expression timed out"), SCRIPT_TIMEOUT_MS);
        worker.onmessage = (event: MessageEvent<{ type: string; value?: unknown; error?: string }>) => {
            if (event.data.type === "evaluated") finish(undefined, event.data.value);
            else if (event.data.type === "evaluationError") finish(event.data.error ?? "evaluation failed");
        };
        worker.onerror = (event) => finish(event.message || "expression worker error");
        worker.postMessage({ type: "evaluate", expression: expr, env });
    });
}

function compare(actual: unknown, op: string, expectedRaw: string): { passed: boolean; error?: string } {
    const expected = coerce(expectedRaw);
    const num = Number(actual);
    const fail = (msg: string) => ({ passed: false, error: msg });
    const pass = { passed: true };
    switch (op) {
        case "eq":
            return actual == expected ? pass : fail(`expected ${fmt(actual)} == ${fmt(expected)}`);
        case "neq":
            return actual != expected ? pass : fail(`expected ${fmt(actual)} != ${fmt(expected)}`);
        case "gt":
            return num > Number(expected) ? pass : fail(`expected ${fmt(actual)} > ${fmt(expected)}`);
        case "gte":
            return num >= Number(expected) ? pass : fail(`expected ${fmt(actual)} >= ${fmt(expected)}`);
        case "lt":
            return num < Number(expected) ? pass : fail(`expected ${fmt(actual)} < ${fmt(expected)}`);
        case "lte":
            return num <= Number(expected) ? pass : fail(`expected ${fmt(actual)} <= ${fmt(expected)}`);
        case "contains":
            return includes(actual, expected) ? pass : fail(`expected ${fmt(actual)} to contain ${fmt(expected)}`);
        case "notContains":
            return !includes(actual, expected) ? pass : fail(`expected ${fmt(actual)} not to contain ${fmt(expected)}`);
        case "in":
            return Array.isArray(expected) && expected.includes(actual) ? pass : fail(`expected ${fmt(actual)} in ${fmt(expected)}`);
        case "notIn":
            return Array.isArray(expected) && !expected.includes(actual) ? pass : fail(`expected ${fmt(actual)} not in ${fmt(expected)}`);
        case "length":
            return (actual as { length?: number })?.length === Number(expected) ? pass : fail(`expected length ${fmt(expected)}`);
        case "matches":
            return new RegExp(String(expected)).test(String(actual)) ? pass : fail(`expected ${fmt(actual)} to match ${fmt(expected)}`);
        case "notMatches":
            return !new RegExp(String(expected)).test(String(actual)) ? pass : fail(`expected ${fmt(actual)} not to match ${fmt(expected)}`);
        case "startsWith":
            return String(actual).startsWith(String(expected)) ? pass : fail(`expected ${fmt(actual)} to start with ${fmt(expected)}`);
        case "endsWith":
            return String(actual).endsWith(String(expected)) ? pass : fail(`expected ${fmt(actual)} to end with ${fmt(expected)}`);
        case "isEmpty":
            return isEmpty(actual) ? pass : fail(`expected ${fmt(actual)} to be empty`);
        case "notEmpty":
            return !isEmpty(actual) ? pass : fail(`expected ${fmt(actual)} not to be empty`);
        case "isNull":
            return actual === null ? pass : fail(`expected null`);
        case "isUndefined":
            return actual === undefined ? pass : fail(`expected undefined`);
        case "isDefined":
            return actual !== undefined ? pass : fail(`expected defined`);
        case "isTruthy":
            return actual ? pass : fail(`expected truthy`);
        case "isFalsy":
            return !actual ? pass : fail(`expected falsy`);
        case "isNumber":
            return typeof actual === "number" ? pass : fail(`expected number`);
        case "isString":
            return typeof actual === "string" ? pass : fail(`expected string`);
        case "isBoolean":
            return typeof actual === "boolean" ? pass : fail(`expected boolean`);
        case "isArray":
            return Array.isArray(actual) ? pass : fail(`expected array`);
        case "between": {
            const parts = expectedRaw.trim().split(/\s+/).map(Number);
            return num >= parts[0] && num <= parts[1] ? pass : fail(`expected ${fmt(actual)} between ${parts[0]} and ${parts[1]}`);
        }
        default:
            return actual == expected ? pass : fail(`expected ${fmt(actual)} == ${fmt(expected)}`);
    }
}

/** Evaluate `assert` rows: lhs is a JS expression over res/req/bru, rhs is `<op> <expected>`. */
export async function evaluateAssertions(
    rows: { name: string; value: string; enabled: boolean }[],
    env: { res?: ResView; req: ReqView; bru: Record<string, unknown> },
    tests: TestResult[],
): Promise<void> {
    for (const a of rows) {
        if (!a.enabled || !a.name.trim()) continue;
        const raw = a.value.trim();
        const sp = raw.split(/\s+/);
        const op = ASSERT_OPS.has(sp[0]) ? sp[0] : "eq";
        const expectedRaw = ASSERT_OPS.has(sp[0]) ? sp.slice(1).join(" ") : raw;
        let actual: unknown;
        try {
            actual = await evaluateExpression(a.name, env);
        } catch (e) {
            tests.push({ name: `${a.name} ${raw}`, passed: false, error: `eval error: ${errMsg(e)}` });
            continue;
        }
        const { passed, error } = compare(actual, op, expectedRaw);
        tests.push({ name: `${a.name} ${raw}`, passed, error });
    }
}
