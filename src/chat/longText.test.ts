import { act, renderHook } from "@testing-library/react";
import { createElement } from "react";
import { describe, expect, it } from "vitest";
import { cutLongText, FoldMemoryContext, newFoldMemory, useLongTextFold, type FoldMemory } from "./longText";

describe("folding a very long message", () => {
    it("leaves a message that is not long alone", () => {
        expect(cutLongText("short", 100)).toBeNull();
        expect(cutLongText("x".repeat(100), 100)).toBeNull();
    });

    it("leaves a message alone when the fold would hide only a sliver of it", () => {
        expect(cutLongText("a".repeat(105), 100)).toBeNull();
    });

    it("cuts at a paragraph break when there is one near the limit", () => {
        const text = `${"a".repeat(80)}\n\n${"b".repeat(80)}`;
        const cut = cutLongText(text, 100);

        expect(cut?.head).toBe("a".repeat(80));
        expect(cut?.hidden).toBe(text.length - 80);
    });

    it("falls back to a line break when there is no paragraph break", () => {
        const text = `${"a".repeat(80)}\n${"b".repeat(80)}`;
        expect(cutLongText(text, 100)?.head).toBe("a".repeat(80));
    });

    it("cuts at the limit when one enormous line has no break at all", () => {
        const cut = cutLongText("a".repeat(500), 100);

        expect(cut?.head).toHaveLength(100);
        expect(cut?.hidden).toBe(400);
    });

    it("closes a code fence the cut would otherwise leave open", () => {
        const text = `intro\n\n\`\`\`ts\n${"const a = 1;\n".repeat(20)}`;
        const cut = cutLongText(text, 120);

        expect(cut?.head.endsWith("\n```")).toBe(true);
        expect((cut!.head.match(/^```/gm) ?? []).length % 2).toBe(0);
    });

    it("closes a tilde fence with tildes", () => {
        const text = `intro\n\n~~~\n${"const a = 1;\n".repeat(20)}`;
        expect(cutLongText(text, 120)?.head.endsWith("\n~~~")).toBe(true);
    });

    it("closes a four-backtick fence with four backticks, ignoring the shorter fence inside it", () => {
        const text = `\`\`\`\`md\n\`\`\`\n${"const a = 1;\n".repeat(20)}`;
        expect(cutLongText(text, 120)?.head.endsWith("\n\`\`\`\`")).toBe(true);
    });

    it("adds nothing when the fence before the cut already closed", () => {
        const text = `\`\`\`ts\nconst a = 1;\n\`\`\`\n\n${"tail ".repeat(60)}`;
        const cut = cutLongText(text, 80);

        expect(cut?.head.endsWith("```")).toBe(false);
        expect((cut!.head.match(/^```/gm) ?? []).length % 2).toBe(0);
    });
});

describe("remembering a fold across a row scrolling away", () => {
    const long = `${"a".repeat(15_000)}\n\n${"b".repeat(15_000)}`;

    function mount(memory: FoldMemory, live: boolean) {
        return renderHook(({ live }) => useLongTextFold("m1-text-0", long, live), {
            initialProps: { live },
            wrapper: ({ children }) => createElement(FoldMemoryContext, { value: memory }, children),
        });
    }

    it("stays expanded after the row remounts", () => {
        const memory = newFoldMemory();
        const first = mount(memory, false);
        expect(first.result.current.cut).not.toBeNull();

        act(() => first.result.current.expand());
        expect(first.result.current.cut).toBeNull();
        first.unmount();

        expect(mount(memory, false).result.current.cut).toBeNull();
    });

    it("never folds a message the reader watched stream in, even after the row remounts", () => {
        const memory = newFoldMemory();
        const streaming = mount(memory, true);
        streaming.rerender({ live: false });
        expect(streaming.result.current.cut).toBeNull();
        streaming.unmount();

        expect(mount(memory, false).result.current.cut).toBeNull();
    });
});
