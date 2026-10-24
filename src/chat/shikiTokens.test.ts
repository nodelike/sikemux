import { describe, expect, it, vi } from "vitest";
import { textMateGrammar, tokenizeCode } from "./shikiTokens";
import { codeThemeName } from "../themes/codeTheme";
import { DEFAULT_THEME_ID } from "../themes";
import { themeById } from "../test/themes";

const invokeCommand = vi.hoisted(() => vi.fn());
vi.mock("../api/invoke", () => ({ invokeCommand }));

/** Answers the way the native side does: the grammar's own JSON, fetched once and kept. */
async function nativeGrammar(_command: string, args: { id: string }): Promise<string> {
    const module = (await import(/* @vite-ignore */ `@shikijs/langs/${args.id}`)) as { default: Array<{ name: string }> };
    return JSON.stringify(module.default.find((grammar) => grammar.name === args.id));
}

const theme = themeById(DEFAULT_THEME_ID);
const name = codeThemeName(theme);

function tokens(lines: Awaited<ReturnType<typeof tokenizeCode>>) {
    return lines.flat();
}

describe("tokenizeCode", () => {
    it("colours code in the palette the diff panes use, and changes none of it", async () => {
        const source = "const answer = 42; // why\nexport default answer;";
        const lines = await tokenizeCode(source, "typescript", theme, name);

        expect(lines.map((line) => line.map((token) => token.text).join("")).join("\n")).toBe(source);
        const word = (text: string) => tokens(lines).find((token) => token.text.includes(text));
        expect(word("const")?.color?.toLowerCase()).toBe(theme.highlight.keyword.toLowerCase());
        expect(word("42")?.color?.toLowerCase()).toBe(theme.highlight.number.toLowerCase());
        expect(word("why")?.color?.toLowerCase()).toBe(theme.highlight.comment.toLowerCase());
        expect(word("why")?.italic).toBe(true);
    });

    it("leaves the plain words of a line uncoloured, so a fence keeps the weight it reads at", async () => {
        const lines = await tokenizeCode("body { color: red; }", "css", theme, name);
        expect(tokens(lines).some((token) => token.color === undefined)).toBe(true);
    });

    it("sets a heading in bold and a link underlined", async () => {
        const lines = await tokenizeCode("# Title\n\nSee [docs](https://example.com)", "markdown", theme, name);
        const word = (text: string) => tokens(lines).find((token) => token.text.includes(text));

        expect(word("Title")?.bold).toBe(true);
        expect(word("https://example.com")?.underline).toBe(true);
    });

    it("says nothing for a grammar there is not", async () => {
        expect(await tokenizeCode("+[-->-[>>+>-----<<]<--<---]", "brainfuck", theme, name)).toEqual([]);
    });

    it("downloads a grammar the app does not ship, along with the grammars it embeds", async () => {
        invokeCommand.mockImplementation(nativeGrammar);
        const lines = await tokenizeCode('---\nconst title = "Home";\n---\n<h1>{title}</h1>', "astro", theme, name);

        expect(invokeCommand).toHaveBeenCalledWith("grammar_load", { id: "astro" });
        expect(invokeCommand).not.toHaveBeenCalledWith("grammar_load", { id: "typescript" });
        const word = (text: string) => tokens(lines).find((token) => token.text.includes(text));
        expect(word("const")?.color?.toLowerCase()).toBe(theme.highlight.keyword.toLowerCase());
    });

    it("asks again after a download fails", async () => {
        invokeCommand.mockReset().mockRejectedValueOnce(new Error("offline")).mockImplementation(nativeGrammar);

        await expect(tokenizeCode("SELECT 1", "sql", theme, name)).rejects.toThrow("offline");
        const lines = await tokenizeCode("SELECT 1", "sql", theme, name);
        expect(tokens(lines).find((token) => token.text.includes("SELECT"))?.color).toBeDefined();
    });
});

describe("textMateGrammar", () => {
    it("hands over a grammar it carries and nothing for one it does not", async () => {
        const grammar = await textMateGrammar("typescript");
        const line = grammar?.tokenizeLine("const a = 1;", null);
        expect(line?.tokens.some((token) => token.scopes.includes("storage.type.ts"))).toBe(true);
        expect(await textMateGrammar("brainfuck")).toBeNull();
    });
});
