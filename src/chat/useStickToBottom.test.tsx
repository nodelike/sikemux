import { act, fireEvent, render } from "@testing-library/react";
import { useRef } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useStickToBottom } from "./useStickToBottom";

let resized: () => void = () => {};
class FakeResizeObserver {
    constructor(callback: () => void) {
        resized = callback;
    }
    observe() {}
    disconnect() {}
}

function Transcript() {
    const scrollRef = useRef<HTMLDivElement>(null);
    const contentRef = useRef<HTMLDivElement>(null);
    const { onScroll } = useStickToBottom({ scrollRef, contentRef, visible: true });
    return (
        <div data-testid="scroller" ref={scrollRef} onScroll={onScroll}>
            <div ref={contentRef}>
                <div className="chat-row" data-testid="reply" />
            </div>
        </div>
    );
}

const box = (top: number, bottom: number) => ({ top, bottom, left: 0, width: 400, height: bottom - top }) as DOMRect;

describe("useStickToBottom", () => {
    afterEach(() => vi.unstubAllGlobals());

    it("keeps the start of a streaming reply in place while the reader is scrolled up in it", () => {
        vi.stubGlobal("ResizeObserver", FakeResizeObserver);
        const { getByTestId } = render(<Transcript />);
        const scroller = getByTestId("scroller");
        const reply = getByTestId("reply");
        scroller.getBoundingClientRect = () => box(0, 600);
        document.elementFromPoint = () => reply;

        let grown = 0;
        const replyTop = () => 100 - grown - (scroller.scrollTop + 500);
        reply.getBoundingClientRect = () => box(replyTop(), 900);
        scroller.scrollTop = -500;
        fireEvent.scroll(scroller);

        grown = 120;
        act(() => resized());

        expect(replyTop()).toBe(100);
    });
});
