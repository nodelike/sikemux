import { act, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ChatPage } from "./ChatPage";
import { ReaderScrollContext } from "./chatAgent";

vi.mock("@tauri-apps/api/core", () => ({ convertFileSrc: (id: string, scheme: string) => `${scheme}://localhost/${id}` }));

const page = { id: "0123456789abcdef0123456789abcdef", title: "Revenue", height: 300 };

function fromFrame(frame: HTMLIFrameElement, data: unknown) {
    act(() => {
        window.dispatchEvent(new MessageEvent("message", { data, source: frame.contentWindow }));
    });
}

describe("ChatPage", () => {
    it("scrolls the transcript for a wheel the page passes on, and grows to the height it reports", () => {
        const scrollByReader = vi.fn();
        const { container } = render(
            <ReaderScrollContext.Provider value={scrollByReader}>
                <ChatPage page={page} />
            </ReaderScrollContext.Provider>,
        );
        const frame = container.querySelector("iframe")!;

        fromFrame(frame, { jsonrpc: "2.0", method: "sikemux/wheel", params: { deltaY: -120 } });
        fromFrame(frame, { jsonrpc: "2.0", method: "ui/notifications/size-changed", params: { height: 640 } });

        expect(scrollByReader).toHaveBeenCalledWith(-120);
        expect((container.firstChild as HTMLElement).style.height).toBe("640px");
    });

    it("ignores messages from anything but its own frame", () => {
        const scrollByReader = vi.fn();
        render(
            <ReaderScrollContext.Provider value={scrollByReader}>
                <ChatPage page={page} />
            </ReaderScrollContext.Provider>,
        );
        window.dispatchEvent(
            new MessageEvent("message", { data: { jsonrpc: "2.0", method: "sikemux/wheel", params: { deltaY: 40 } }, source: window }),
        );
        expect(scrollByReader).not.toHaveBeenCalled();
    });
});
