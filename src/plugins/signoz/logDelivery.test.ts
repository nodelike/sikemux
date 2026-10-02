import { describe, expect, it } from "vitest";
import type { LogLine } from "./api";
import { logDelivery } from "./logDelivery";

function line(overrides: Partial<LogLine>): LogLine {
    return {
        id: "a",
        timestamp: "2026-09-24T09:11:54.569Z",
        service: "api-gateway",
        severity: "ERROR",
        body: "upstream timed out",
        traceId: null,
        spanId: null,
        attributes: {},
        resources: {},
        ...overrides,
    };
}

describe("logDelivery", () => {
    it("puts each line's time, level, service, trace and body in one block", () => {
        const text = logDelivery([line({ traceId: "16db" }), line({ id: "b", service: null, severity: null, body: "retry\n  at handler" })]).text;
        expect(text).toBe(
            "2 log lines from SigNoz:\n\n```\n" +
                "2026-09-24T09:11:54.569Z ERROR api-gateway trace=16db\nupstream timed out\n\n" +
                "2026-09-24T09:11:54.569Z - -\nretry\n  at handler\n```\n",
        );
    });

    it("reads naturally for a single line", () => {
        expect(logDelivery([line({})]).text?.startsWith("A log line from SigNoz:\n")).toBe(true);
    });
});
