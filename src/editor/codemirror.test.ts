import { toggleComment } from "@codemirror/commands";
import { ensureSyntaxTree } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { classHighlighter, highlightTree } from "@lezer/highlight";
import { describe, expect, it, vi } from "vitest";
import { languageFor, loadLanguage } from "./codemirror";

const invokeCommand = vi.hoisted(() => vi.fn());
vi.mock("../api/invoke", () => ({ invokeCommand }));

/** Answers the way the native side does: the grammar's own JSON. */
async function nativeGrammar(_command: string, args: { id: string }): Promise<string> {
    const module = (await import(/* @vite-ignore */ `@shikijs/langs/${args.id}`)) as { default: Array<{ name: string }> };
    return JSON.stringify(module.default.find((grammar) => grammar.name === args.id));
}

describe("editor languages", () => {
    it("highlights dotenv keys, values, and comments", async () => {
        const state = EditorState.create({
            doc: "FIRST=one\n# explanation",
            extensions: await loadLanguage(".env"),
        });
        const tree = ensureSyntaxTree(state, state.doc.length, 100);
        const spans: Array<{ text: string; classes: string }> = [];

        expect(tree).not.toBeNull();
        highlightTree(tree!, classHighlighter, (from, to, classes) => spans.push({ text: state.sliceDoc(from, to), classes }));

        expect(spans).toEqual(
            expect.arrayContaining([
                { text: "FIRST", classes: expect.stringContaining("tok-variableName") },
                { text: "one", classes: "tok-string" },
                { text: "# explanation", classes: "tok-comment" },
            ]),
        );
    });

    it.each([".env", ".env.local"])("toggles selected lines in %s files", async (path) => {
        const doc = "FIRST=one\nSECOND=two";
        let state = EditorState.create({
            doc,
            selection: { anchor: 0, head: doc.length },
            extensions: await loadLanguage(path),
        });

        expect(toggleComment({ state, dispatch: (transaction) => (state = transaction.state) })).toBe(true);
        expect(state.doc.toString()).toBe("# FIRST=one\n# SECOND=two");

        expect(toggleComment({ state, dispatch: (transaction) => (state = transaction.state) })).toBe(true);
        expect(state.doc.toString()).toBe(doc);
    });

    it("highlights OpenSSH config structure", async () => {
        const state = EditorState.create({
            doc: "Host staging\n  HostName staging.example.com\n  Port 2222\n  CanonicalizeHostname yes\n  ProxyCommand ssh -W %h:%p jump\n# note",
            extensions: await loadLanguage("/Users/me/.ssh/config"),
        });
        const tree = ensureSyntaxTree(state, state.doc.length, 100);
        const spans: Array<{ text: string; classes: string }> = [];

        expect(tree).not.toBeNull();
        highlightTree(tree!, classHighlighter, (from, to, classes) => spans.push({ text: state.sliceDoc(from, to), classes }));

        expect(spans).toEqual(
            expect.arrayContaining([
                { text: "Host", classes: expect.stringContaining("tok-keyword") },
                { text: "HostName", classes: expect.stringContaining("tok-propertyName") },
                { text: "2222", classes: expect.stringContaining("tok-number") },
                { text: "yes", classes: expect.stringContaining("tok-bool") },
                { text: "%h", classes: expect.stringContaining("tok-variableName") },
                { text: "# note", classes: expect.stringContaining("tok-comment") },
            ]),
        );
    });

    it("honours the SSH config language hint independently of path", async () => {
        const state = EditorState.create({
            doc: "Host production\n  HostName prod.example.com",
            extensions: await loadLanguage("/tmp/config", "ssh-config"),
        });
        const tree = ensureSyntaxTree(state, state.doc.length, 100);
        const spans: Array<{ text: string; classes: string }> = [];

        expect(tree).not.toBeNull();
        highlightTree(tree!, classHighlighter, (from, to, classes) => spans.push({ text: state.sliceDoc(from, to), classes }));
        expect(spans).toEqual(
            expect.arrayContaining([
                { text: "Host", classes: expect.stringContaining("tok-keyword") },
                { text: "HostName", classes: expect.stringContaining("tok-propertyName") },
            ]),
        );
    });

    it("highlights a file it has no language for with the grammar diffs and chat use", async () => {
        invokeCommand.mockImplementation(nativeGrammar);
        const doc = '---\nconst title = "Home";\n---\n<h1 class="big">{title}</h1>\n';
        const state = EditorState.create({ doc, extensions: await loadLanguage("src/pages/index.astro") });
        const tree = ensureSyntaxTree(state, state.doc.length, 1000);
        const spans: Array<{ text: string; classes: string }> = [];

        expect(tree).not.toBeNull();
        highlightTree(tree!, classHighlighter, (from, to, classes) => spans.push({ text: state.sliceDoc(from, to), classes }));

        expect(spans).toEqual(
            expect.arrayContaining([
                { text: "const", classes: expect.stringContaining("tok-keyword") },
                { text: '"Home"', classes: expect.stringContaining("tok-string") },
                { text: "h1", classes: expect.stringContaining("tok-typeName") },
                { text: "class", classes: expect.stringContaining("tok-propertyName") },
            ]),
        );
    });

    it("tries a downloaded grammar again after it failed to arrive", async () => {
        invokeCommand.mockReset().mockRejectedValueOnce(new Error("offline")).mockImplementation(nativeGrammar);

        await expect(loadLanguage("/repo/build.zig")).rejects.toThrow("offline");
        expect((await loadLanguage("/repo/build.zig")).length).toBeGreaterThan(0);
    });
});

describe("language loading", () => {
    it("hands back the same grammar to every document in that language", async () => {
        const first = await loadLanguage("/repo/a.ts");
        const second = await loadLanguage("/repo/b.mts");
        expect(second).toBe(first);
        expect(languageFor("/repo/c.cts")).toBe(first);
    });

    it("reports nothing for a path with no grammar", async () => {
        expect(await loadLanguage("/repo/notes.unknownext")).toEqual([]);
        expect(languageFor("/repo/notes.unknownext")).toEqual([]);
    });

    it("has no grammar to offer until the pack has downloaded", () => {
        expect(languageFor("/repo/main.lua")).toEqual([]);
    });
});
