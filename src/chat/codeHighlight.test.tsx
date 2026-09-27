import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CodeTokens, fenceLanguage, splitAtMark, useCodeTokens, useDiffTokens } from "./codeHighlight";
import type { CodeLine } from "./types";
import type { DiffLine } from "./diff";

const tokenizeCode = vi.fn<(text: string, lang: string) => Promise<CodeLine[]>>();

vi.mock("./shikiTokens", () => ({
    tokenizeCode: (text: string, lang: string) => tokenizeCode(text, lang),
}));

function Fence({ text, lang }: { text: string; lang: string | null }) {
    const lines = useCodeTokens(text, lang);
    return <pre data-testid="fence">{lines ? <CodeTokens lines={lines} /> : text}</pre>;
}

const coloured = (text: string): CodeLine[] => text.split("\n").map((line) => [{ text: line, color: "#c792ea" }]);

beforeEach(() => {
    vi.useFakeTimers();
    tokenizeCode.mockImplementation((text) => Promise.resolve(coloured(text)));
});

afterEach(() => {
    cleanup();
    vi.useRealTimers();
    vi.clearAllMocks();
});

/** Lets the settle timer fire and the tokens come back. */
async function settle() {
    await act(async () => {
        await vi.advanceTimersByTimeAsync(400);
    });
}

describe("fenceLanguage", () => {
    it("reads the word after the backticks, or the file the fence names", () => {
        expect(fenceLanguage("ts")).toBe("typescript");
        expect(fenceLanguage("TSX")).toBe("typescript");
        expect(fenceLanguage("src/styles/chat.css")).toBe("css");
        expect(fenceLanguage("src/chat/AgentChatPane.tsx:412")).toBe("typescript");
        expect(fenceLanguage("bash")).toBe("shellscript");
        expect(fenceLanguage("src/pages/index.astro")).toBe("astro");
    });

    it("leaves a fence alone when the grammar is not one we carry", () => {
        expect(fenceLanguage(undefined)).toBeNull();
        expect(fenceLanguage("")).toBeNull();
        expect(fenceLanguage("brainfuck")).toBeNull();
        expect(fenceLanguage("notes.txt")).toBeNull();
    });
});

describe("useCodeTokens", () => {
    it("colours a fence once it settles and keeps the text it was given", async () => {
        render(<Fence text={"const value = 1;\nexport default value;"} lang="typescript" />);
        expect(tokenizeCode).not.toHaveBeenCalled();

        await settle();

        expect(tokenizeCode).toHaveBeenCalledOnce();
        expect(screen.getByTestId("fence")).toHaveTextContent("const value = 1; export default value;");
        expect(screen.getByTestId("fence").querySelector("span")).toHaveStyle({ color: "#c792ea" });
    });

    /* A message still being written re-reads its markdown ten times a second,
       and the fence at the end of it is longer every time. */
    it("leaves a fence that is still being written alone until it stops growing", async () => {
        const { rerender } = render(<Fence text="const a" lang="typescript" />);
        for (const text of ["const a =", "const a = 1", "const a = 1;"]) {
            await act(async () => {
                await vi.advanceTimersByTimeAsync(100);
            });
            rerender(<Fence text={text} lang="typescript" />);
        }
        expect(tokenizeCode).not.toHaveBeenCalled();

        await settle();
        expect(tokenizeCode).toHaveBeenCalledOnce();
        expect(tokenizeCode).toHaveBeenCalledWith("const a = 1;", "typescript");
    });

    it("reads a fence it has already coloured out of the cache", async () => {
        const source = "const cached = true;";
        const { unmount } = render(<Fence text={source} lang="typescript" />);
        await settle();
        expect(tokenizeCode).toHaveBeenCalledOnce();
        unmount();

        render(<Fence text={source} lang="typescript" />);
        // A row coming back on screen paints coloured on its first frame.
        expect(screen.getByTestId("fence").querySelector("span")).toHaveStyle({ color: "#c792ea" });
        await settle();
        expect(tokenizeCode).toHaveBeenCalledOnce();
    });

    it("never reads a block too long to be worth colouring", async () => {
        render(<Fence text={"x = 1;\n".repeat(151)} lang="typescript" />);
        await settle();
        expect(tokenizeCode).not.toHaveBeenCalled();

        cleanup();
        render(<Fence text={"y".repeat(6_001)} lang="typescript" />);
        await settle();
        expect(tokenizeCode).not.toHaveBeenCalled();

        cleanup();
        render(<Fence text={"x = 1;\n".repeat(120)} lang="typescript" />);
        await settle();
        expect(tokenizeCode).toHaveBeenCalledOnce();
    });

    it("leaves a fence with no grammar as plain text", async () => {
        render(<Fence text="hello" lang={null} />);
        await settle();
        expect(tokenizeCode).not.toHaveBeenCalled();
        expect(screen.getByTestId("fence").querySelector("span")).toBeNull();
    });
});

function Diff({ lines, path }: { lines: DiffLine[]; path: string }) {
    const coloured = useDiffTokens(lines, path);
    return (
        <div data-testid="diff">
            {lines.map((line, index) => (
                <span key={index} data-read={coloured?.get(line)?.[0]?.text ?? ""}>
                    {line.text}
                </span>
            ))}
        </div>
    );
}

/* The same place in a file before and after a change: read as one document the
   two versions of the middle line would follow each other, which they never do
   in the file either of them came from. */
const hunk: DiffLine[] = [
    { sign: " ", text: "const a = 1;" },
    { sign: "-", text: "const b = 2;", mark: [10, 11] },
    { sign: "+", text: "const b = 3;", mark: [10, 11] },
    { sign: " ", text: "export {};" },
];

describe("useDiffTokens", () => {
    it("reads each side of a diff as the file it came from", async () => {
        render(<Diff lines={hunk} path="/repo/src/thing.ts" />);
        await settle();

        expect(tokenizeCode.mock.calls.map(([text]) => text)).toEqual([
            "const a = 1;\nconst b = 2;\nexport {};",
            "const a = 1;\nconst b = 3;\nexport {};",
        ]);
        // Every line is coloured by its own words, which only holds if the two
        // sides were read apart and put back line by line.
        for (const span of screen.getByTestId("diff").querySelectorAll("span")) {
            expect(span.getAttribute("data-read")).toBe(span.textContent);
        }
    });

    it("leaves a diff to a file it has no grammar for alone", async () => {
        render(<Diff lines={hunk} path="/repo/notes.txt" />);
        await settle();
        expect(tokenizeCode).not.toHaveBeenCalled();
    });
});

describe("splitAtMark", () => {
    const line: CodeLine = [
        { text: "const b = ", color: "#a" },
        { text: "2", color: "#b" },
        { text: ";", color: "#c" },
    ];
    const said = (tokens: CodeLine) => tokens.map((token) => token.text).join("");

    it("hands back the whole line when nothing changed inside it", () => {
        const { pre, marked, post } = splitAtMark(line);
        expect(said(pre)).toBe("const b = 2;");
        expect(marked).toEqual([]);
        expect(post).toEqual([]);
    });

    it("cuts the runs the changed span crosses at its edges", () => {
        const { pre, marked, post } = splitAtMark(line, [6, 11]);
        expect(said(pre)).toBe("const ");
        expect(said(marked)).toBe("b = 2");
        expect(said(post)).toBe(";");
        // The run a cut lands in keeps its colour on both sides of it.
        expect(pre.at(-1)?.color).toBe("#a");
        expect(marked[0]?.color).toBe("#a");
        expect(marked.at(-1)?.color).toBe("#b");
    });

    it("keeps every character of the line, in order", () => {
        for (const mark of [
            [0, 1],
            [0, 12],
            [3, 4],
            [10, 11],
            [11, 12],
        ] as [number, number][]) {
            const { pre, marked, post } = splitAtMark(line, mark);
            expect(said(pre) + said(marked) + said(post)).toBe("const b = 2;");
            expect(said(marked)).toBe("const b = 2;".slice(mark[0], mark[1]));
        }
    });
});
