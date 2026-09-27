import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { MarkdownRequest } from "../api/markdown";
import type { MarkdownOptions, MdElement } from "./types";

const mocks = vi.hoisted(() => ({ parse: vi.fn() }));
vi.mock("../api/markdown", () => ({ markdownApi: { parse: mocks.parse } }));

const { forgetMarkdownForTests, settleBlocks, useMarkdownBlocks } = await import("./useMarkdownBlocks");

const OPTIONS: MarkdownOptions = { gfm: true, htmlAsText: false, fileLinks: true };

function paragraphs(text: string): MdElement[] {
    return text.split("\n\n").map((part) => ({ t: "p", c: [part] }));
}

function answer(request: MarkdownRequest): Promise<MdElement[]> {
    return Promise.resolve(paragraphs(request.text).slice(request.skip));
}

let drawn: readonly MdElement[] | null = null;

function Probe({ text, live }: { text: string; live: boolean }) {
    drawn = useMarkdownBlocks(text, OPTIONS, live);
    return <output>{drawn ? drawn.length : "unread"}</output>;
}

beforeEach(() => {
    forgetMarkdownForTests();
    mocks.parse.mockReset();
    mocks.parse.mockImplementation(answer);
    drawn = null;
});

afterEach(cleanup);

describe("reading a message into blocks", () => {
    it("asks only for the end of a message still being written, and keeps the blocks it has", async () => {
        const { rerender } = render(<Probe text={"Plan\n\nStep one\n\nStep two"} live />);
        await waitFor(() => expect(drawn).toHaveLength(3));
        expect(mocks.parse).toHaveBeenLastCalledWith(expect.objectContaining({ skip: 0 }));
        const [plan, one] = drawn!;

        rerender(<Probe text={"Plan\n\nStep one\n\nStep two, then three"} live />);
        await waitFor(() => expect(drawn![2]).toEqual({ t: "p", c: ["Step two, then three"] }));
        expect(mocks.parse).toHaveBeenLastCalledWith(expect.objectContaining({ skip: 1 }));
        expect(drawn![0]).toBe(plan);
        expect(drawn![1]).toBe(one);
    });

    it("reads a finished message in full, and keeps every block that did not change", async () => {
        const { rerender } = render(<Probe text={"Plan\n\nDone"} live />);
        await waitFor(() => expect(drawn).toHaveLength(2));
        const before = drawn!;

        rerender(<Probe text={"Plan\n\nDone"} live={false} />);
        await waitFor(() => expect(mocks.parse).toHaveBeenCalledTimes(2));
        expect(mocks.parse).toHaveBeenLastCalledWith(expect.objectContaining({ skip: 0 }));
        await waitFor(() => expect(drawn).not.toBe(before));
        expect(drawn![0]).toBe(before[0]);
        expect(drawn![1]).toBe(before[1]);
    });

    it("reads one version at a time, and then the newest text as a whole", async () => {
        let release: () => void = () => {};
        mocks.parse.mockImplementationOnce(
            (request: MarkdownRequest) => new Promise<MdElement[]>((resolve) => (release = () => resolve(paragraphs(request.text)))),
        );
        const { rerender } = render(<Probe text="a" live />);
        await waitFor(() => expect(mocks.parse).toHaveBeenCalledTimes(1));
        rerender(<Probe text="ab" live />);
        rerender(<Probe text="abc" live />);
        expect(mocks.parse).toHaveBeenCalledTimes(1);

        await act(async () => release());
        await waitFor(() => expect(mocks.parse).toHaveBeenCalledTimes(2));
        expect(mocks.parse).toHaveBeenLastCalledWith(expect.objectContaining({ text: "abc" }));
        await waitFor(() => expect(drawn).toEqual([{ t: "p", c: ["abc"] }]));
    });

    it("draws a finished message the moment it mounts again", async () => {
        const { unmount } = render(<Probe text={"Kept\n\nfor later"} live={false} />);
        await waitFor(() => expect(drawn).toHaveLength(2));
        unmount();

        render(<Probe text={"Kept\n\nfor later"} live={false} />);
        expect(screen.getByRole("status")).toHaveTextContent("2");
        expect(mocks.parse).toHaveBeenCalledTimes(1);
    });

    it("shows the words as written when the parser cannot be reached", async () => {
        mocks.parse.mockRejectedValueOnce(new Error("no native side"));
        render(<Probe text="**still** readable" live={false} />);
        await waitFor(() => expect(drawn).toEqual([{ t: "p", c: ["**still** readable"] }]));
    });
});

describe("settling blocks", () => {
    it("keeps the blocks before the ones asked for, and any returned unchanged", () => {
        const previous: MdElement[] = [
            { t: "p", c: ["a"] },
            { t: "ul", c: [{ t: "li", c: ["b"] }] },
            { t: "p", c: ["c"] },
        ];
        const next = settleBlocks(previous, 1, [
            { t: "ul", c: [{ t: "li", c: ["b"] }] },
            { t: "p", c: ["c d"] },
        ]);
        expect(next[0]).toBe(previous[0]);
        expect(next[1]).toBe(previous[1]);
        expect(next[2]).toEqual({ t: "p", c: ["c d"] });
    });
});
