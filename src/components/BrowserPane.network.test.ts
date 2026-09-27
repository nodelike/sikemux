import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { beforeEach, describe, expect, it, vi } from "vitest";

/* The recorder is injected into every browser tab before the page's own
   scripts. It is plain interception with no layout in it, so it can be
   exercised here against stand-ins for fetch and XHR. */
const RECORDER = readFileSync(resolve(__dirname, "../../src-tauri/src/browser/recorder.js"), "utf8");

interface Entry {
    kind: string;
    method: string;
    url: string;
    status: number | null;
    statusText: string;
    contentType: string;
    durationMs: number | null;
    requestBody: string | null;
    body: string | null;
    error: string | null;
    framework?: string;
}

function response(status: number, contentType: string, text: string) {
    return {
        status,
        statusText: status === 200 ? "OK" : "Bad Request",
        headers: { get: (name: string) => (name.toLowerCase() === "content-type" ? contentType : null) },
        clone: () => ({ text: () => Promise.resolve(text) }),
    };
}

class FakeXhr {
    method = "";
    url = "";
    status = 0;
    statusText = "";
    responseType = "";
    responseText = "";
    contentType = "";
    listeners: Record<string, Array<() => void>> = {};
    open(method: string, url: string) {
        this.method = method;
        this.url = url;
    }
    send(_body?: unknown) {}
    addEventListener(type: string, listener: () => void) {
        (this.listeners[type] ??= []).push(listener);
    }
    getResponseHeader(name: string) {
        return name.toLowerCase() === "content-type" ? this.contentType : null;
    }
    fail(type: "error" | "abort" | "timeout") {
        for (const listener of [...(this.listeners[type] ?? []), ...(this.listeners.loadend ?? [])]) listener();
    }
    settle(status: number, contentType: string, text: string) {
        this.status = status;
        this.statusText = status === 200 ? "OK" : "Server Error";
        this.contentType = contentType;
        this.responseText = text;
        for (const listener of this.listeners.loadend ?? []) listener();
    }
}

const net = () => (window as unknown as { __sikemuxNet: { entries: () => Entry[] } }).__sikemuxNet;

let fetched: Array<{ input: unknown; init: unknown }>;
/* Swapped by a test instead of reassigning window.fetch: the recorder wraps
   whatever fetch it found at install time, so replacing it would lose the wrap. */
let answer: () => Promise<unknown>;

beforeEach(() => {
    delete (window as unknown as Record<string, unknown>).__sikemuxNet;
    fetched = [];
    vi.stubGlobal("XMLHttpRequest", FakeXhr);
    answer = () => Promise.resolve(response(200, "application/json", '{"ok":true}'));
    window.fetch = ((input: unknown, init: unknown) => {
        fetched.push({ input, init });
        return answer();
    }) as unknown as typeof window.fetch;
    (0, eval)(RECORDER);
});

describe("browser network recorder", () => {
    it("records a fetch with its status and answer, and hands the page the untouched response", async () => {
        const result = await window.fetch("https://api.test/plan", { method: "post", body: '{"draft":"a51"}' });

        expect(result.status).toBe(200);
        expect(fetched).toHaveLength(1);
        await vi.waitFor(() => expect(net().entries()[0].body).toBe('{"ok":true}'));
        const [entry] = net().entries();
        expect(entry).toMatchObject({
            kind: "fetch",
            method: "POST",
            url: "https://api.test/plan",
            status: 200,
            statusText: "OK",
            contentType: "application/json",
            requestBody: '{"draft":"a51"}',
            error: null,
        });
        expect(entry.durationMs).toBeGreaterThanOrEqual(0);
    });

    it("leaves a binary answer unread but still reports that the call happened", async () => {
        answer = () => Promise.resolve(response(200, "image/png", "not text"));
        await window.fetch("https://api.test/thumb.png");

        const [entry] = net().entries();
        expect(entry.status).toBe(200);
        expect(entry.contentType).toBe("image/png");
        expect(entry.body).toBeNull();
    });

    it("keeps a failed request, which is how a dead server reads", async () => {
        answer = () => Promise.reject(new Error("Load failed"));
        await expect(window.fetch("https://api.test/plan")).rejects.toThrow("Load failed");

        expect(net().entries()[0]).toMatchObject({ url: "https://api.test/plan", status: null, error: "Error: Load failed" });
    });

    it("says why an xhr never got an answer", () => {
        const request = new (window as unknown as { XMLHttpRequest: typeof FakeXhr }).XMLHttpRequest();
        request.open("get", "https://api.test/build");
        request.send();
        request.fail("error");
        expect(net().entries()[0]).toMatchObject({ status: 0, error: "network error" });
    });

    it("names the data requests frameworks make behind a navigation", async () => {
        await window.fetch("https://app.test/orders?_rsc=1x2y", { headers: { RSC: "1" } });
        await window.fetch("https://app.test/orders", { method: "POST", headers: new Headers({ "Next-Action": "abc" }) });
        await window.fetch("https://app.test/_next/data/build/orders.json");
        await window.fetch("https://app.test/api/orders");

        expect(
            net()
                .entries()
                .map((entry) => entry.framework),
        ).toEqual(["next rsc", "next server action", "next data", undefined]);
    });

    it("records an xhr once it settles", () => {
        const request = new (window as unknown as { XMLHttpRequest: typeof FakeXhr }).XMLHttpRequest();
        request.open("get", "https://api.test/build");
        request.send();
        expect(net().entries()[0]).toMatchObject({ kind: "xhr", method: "GET", status: null });

        request.settle(500, "text/plain", "boom");
        expect(net().entries()[0]).toMatchObject({ status: 500, statusText: "Server Error", body: "boom" });
    });

    it("holds a bounded history so a long-lived page cannot grow without end", async () => {
        for (let index = 0; index < 130; index += 1) await window.fetch(`https://api.test/${index}`);

        const entries = net().entries();
        expect(entries).toHaveLength(120);
        expect(entries.at(-1)?.url).toBe("https://api.test/129");
        expect(entries[0].url).toBe("https://api.test/10");
    });
});
