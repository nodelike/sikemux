import { describe, expect, it, vi } from "vitest";
import { tokenizeCode, tokenizeLines } from "./shikiTokens";
import { codeThemeName } from "../themes/codeTheme";
import { DEFAULT_THEME_ID, themeById } from "../themes";

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

describe("tokenizeLines", () => {
    const never = { maxLineLength: 1000, stale: () => false };

    it("carries the grammar's state from one slice to the next", async () => {
        const lines = [...Array.from({ length: 199 }, (_, n) => `const v${n} = ${n};`), "/* a comment", "that keeps going", "*/ const after = 1;"];
        const coloured = await tokenizeLines(lines, "typescript", theme, name, never);

        expect(coloured).toHaveLength(lines.length);
        expect(coloured?.[200].map((token) => token.text).join("")).toBe("that keeps going");
        expect(coloured?.[200][0].color?.toLowerCase()).toBe(theme.highlight.comment.toLowerCase());
    });

    it("gives up once the answer is no longer wanted, and has nothing for a grammar there is not", async () => {
        const lines = Array.from({ length: 450 }, (_, n) => `const v${n} = ${n};`);
        let calls = 0;
        expect(await tokenizeLines(lines, "typescript", theme, name, { maxLineLength: 1000, stale: () => ++calls > 1 })).toBeNull();
        expect(await tokenizeLines(["+[-]"], "brainfuck", theme, name, never)).toBeNull();
    });
});
