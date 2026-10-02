import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { beforeEach, describe, expect, it } from "vitest";

/* The reader an agent's browser_network and browser_wait call to look at
   what the recorder kept. It evaluates to an object of functions. */
const RECORDS = readFileSync(resolve(__dirname, "../../src-tauri/src/browser/records.js"), "utf8");

interface Call {
    id: number;
    method: string;
    url: string;
    status: number | null;
    durationMs: number | null;
    startedAt: number;
    body: string | null;
    requestBody: string | null;
    error: string | null;
    bodyLength?: number;
}

interface Records {
    network(
        limit?: number | null,
        filter?: string | null,
        method?: string | null,
        status?: string | null,
        id?: number | null,
    ): {
        calls?: Call[];
        call?: Call;
        matched?: number;
        omitted?: string;
        note?: string;
    };
    pending(): number | null;
}

let entries: Call[] = [];
const records = () => (0, eval)(RECORDS) as Records;
const call = (id: number, url: string, extra: Partial<Call> = {}): Call => ({
    id,
    method: "GET",
    url,
    status: 200,
    durationMs: 12,
    startedAt: performance.now(),
    body: null,
    requestBody: null,
    error: null,
    ...extra,
});

beforeEach(() => {
    entries = [];
    (window as unknown as Record<string, unknown>).__sikemuxNet = { entries: () => entries };
});

describe("browser network reader", () => {
    it("filters on the path before the query string, so a beacon naming the page does not match", () => {
        entries = [call(1, "https://app.test/api/applications"), call(2, "https://analytics.test/collect?page=applications")];

        expect(
            records()
                .network(null, "applications")
                .calls!.map((entry) => entry.id),
        ).toEqual([1]);
        expect(
            records()
                .network(null, "page=applications")
                .calls!.map((entry) => entry.id),
        ).toEqual([2]);
    });

    it("narrows by method and by status code, class or failure", () => {
        entries = [
            call(1, "https://a.test/x", { method: "POST", status: 201 }),
            call(2, "https://a.test/y", { status: 404 }),
            call(3, "https://a.test/z", { status: null, error: "TypeError: Load failed" }),
            call(4, "https://a.test/w", { status: null, durationMs: null }),
        ];
        const ids = (method: string | null, status: string | null) =>
            records()
                .network(null, null, method, status)
                .calls!.map((entry) => entry.id);

        expect(ids("post", null)).toEqual([1]);
        expect(ids(null, "404")).toEqual([2]);
        expect(ids(null, "2xx")).toEqual([1]);
        expect(ids(null, "failed")).toEqual([2, 3]);
        expect(ids(null, "pending")).toEqual([4]);
    });

    it("cuts bodies in a list, stays under its size budget, and hands over one call whole by id", () => {
        const body = "x".repeat(4000);
        entries = Array.from({ length: 60 }, (_, index) => call(index + 1, `https://a.test/${index}`, { body }));

        const listed = records().network(60);
        expect(listed.calls![0].body).toHaveLength(301);
        expect(listed.calls![0].bodyLength).toBe(4000);
        expect(JSON.stringify(listed.calls).length).toBeLessThan(24000);
        expect(listed.omitted).toMatch(/older calls left out/);
        expect(listed.calls!.at(-1)!.id).toBe(60);
        expect(records().network(null, null, null, null, 7).call!.body).toHaveLength(4000);
        expect(() => records().network(null, null, null, null, 999)).toThrow(/no call 999/);
    });

    it("counts calls still waiting, leaving out ones open long enough to be a stream", () => {
        const now = performance.now();
        entries = [
            call(1, "https://a.test/a", { durationMs: null, startedAt: now }),
            call(2, "https://a.test/b", { durationMs: null, startedAt: now - 60000 }),
            call(3, "https://a.test/c"),
        ];

        expect(records().pending()).toBe(1);
    });
});
