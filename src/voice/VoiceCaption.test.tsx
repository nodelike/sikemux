import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { captionStyle, useFollowNewest } from "./VoiceCaption";

function box(element: HTMLElement, rect: { left: number; top: number; width: number; height: number }) {
    element.getBoundingClientRect = () => ({
        ...rect,
        right: rect.left + rect.width,
        bottom: rect.top + rect.height,
        x: rect.left,
        y: rect.top,
        toJSON: () => rect,
    });
    return element;
}

afterEach(() => {
    cleanup();
    document.body.innerHTML = "";
});

describe("captionStyle", () => {
    it("sits inside the bottom of any other field", () => {
        const field = box(document.createElement("textarea"), { left: 0, top: 500, width: 400, height: 100 });
        document.body.append(field);
        expect(captionStyle(field, 800)).toEqual({ left: 200, bottom: 800 - 600 + 12, maxWidth: 376 });
    });

    it("centres near the bottom of the window with no field to follow", () => {
        expect(captionStyle(null, 800)).toEqual({ left: "50%", bottom: 64 });
        expect(captionStyle(document.createElement("div"), 800)).toEqual({ left: "50%", bottom: 64 });
    });
});

describe("useFollowNewest", () => {
    function Caption({ text }: { text: string }) {
        const { lines, onScroll } = useFollowNewest(text);
        return (
            <div data-testid="lines" ref={lines} onScroll={onScroll}>
                {text}
            </div>
        );
    }

    function sized(element: HTMLElement, scrollHeight: number, clientHeight = 40) {
        Object.defineProperty(element, "scrollHeight", { configurable: true, value: scrollHeight });
        Object.defineProperty(element, "clientHeight", { configurable: true, value: clientHeight });
    }

    it("scrolls to the newest words as they arrive, until the person scrolls up", () => {
        const { rerender } = render(<Caption text="one" />);
        const lines = screen.getByTestId("lines");
        sized(lines, 120);
        rerender(<Caption text="one two three" />);
        expect(lines.scrollTop).toBe(120);

        lines.scrollTop = 10;
        fireEvent.scroll(lines);
        sized(lines, 200);
        rerender(<Caption text="one two three four" />);
        expect(lines.scrollTop).toBe(10);

        lines.scrollTop = 160;
        fireEvent.scroll(lines);
        sized(lines, 240);
        rerender(<Caption text="one two three four five" />);
        expect(lines.scrollTop).toBe(240);
    });
});
