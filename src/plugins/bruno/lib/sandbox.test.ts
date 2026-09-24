import { describe, expect, it, vi } from "vitest";
import { constrainWorkerRequest, evaluateExpression, runScript, type SandboxCtx } from "./sandbox";

const trust = {
    allow_private_network: false,
    allow_file_read: false,
    allow_insecure_tls: false,
    file_root: null,
};

function ctx(): SandboxCtx {
    return {
        vars: { token: "old" },
        req: { method: "GET", url: "https://example.test", headers: {}, body: null, name: "request" },
        res: { status: 200, statusText: "OK", headers: { "content-type": "application/json" }, body: { ok: true }, responseTime: 42 },
        logs: [],
        tests: [],
        trust,
    };
}

describe("Bruno script isolation", () => {
    it("fails closed instead of evaluating in the renderer when workers are unavailable", async () => {
        const original = globalThis.Worker;
        vi.stubGlobal("Worker", undefined);
        const c = ctx();
        await runScript("globalThis.__rendererPwned = true", c);
        expect((globalThis as typeof globalThis & { __rendererPwned?: boolean }).__rendererPwned).toBeUndefined();
        expect(c.logs[0]?.text).toContain("isolated script worker unavailable");
        await expect(evaluateExpression("res.status", { res: c.res, req: c.req, bru: {} })).rejects.toThrow("isolated expression worker unavailable");
        vi.stubGlobal("Worker", original);
    });

    it("does not let worker messages escalate collection trust", () => {
        const request = constrainWorkerRequest(
            {
                method: "GET",
                url: "http://127.0.0.1",
                headers: [],
                body: { kind: "none" },
                timeout_ms: 0,
                skip_tls_verify: true,
                trust: {
                    allow_private_network: true,
                    allow_file_read: true,
                    allow_insecure_tls: true,
                    file_root: "/",
                },
            },
            trust,
        );
        expect(request.trust).toEqual(trust);
        expect(request.trust).not.toBe(trust);
    });
});
