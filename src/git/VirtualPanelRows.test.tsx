import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, onTestFinished, vi } from "vitest";
import { VirtualPanelRows } from "./VirtualPanelRows";

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

describe("VirtualPanelRows", () => {
    it("renders small lists without a virtual scroll wrapper", () => {
        render(
            <div>
                <VirtualPanelRows items={["one", "two"]} selectedIndex={0} focused getKey={(item) => item} renderRow={(item) => <div>{item}</div>} />
            </div>,
        );
        expect(screen.getByText("one")).toBeInTheDocument();
        expect(screen.getByText("two")).toBeInTheDocument();
        expect(document.querySelector(".git-virtual-rows")).toBeNull();
    });

    it("mounts only the rows near the view of a long list, and scrolls to the selected one", async () => {
        vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(260);
        vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(300);
        vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(300 * 26);
        const scrollTo = vi.fn();
        const original = HTMLElement.prototype.scrollTo;
        HTMLElement.prototype.scrollTo = scrollTo;
        onTestFinished(() => {
            HTMLElement.prototype.scrollTo = original;
        });
        const items = Array.from({ length: 300 }, (_, i) => `row ${i}`);
        const list = (selectedIndex: number, focused: boolean) => (
            <div style={{ overflow: "auto" }}>
                <VirtualPanelRows
                    items={items}
                    selectedIndex={selectedIndex}
                    focused={focused}
                    estimateSize={26}
                    getKey={(item) => item}
                    renderRow={(item) => <div>{item}</div>}
                />
            </div>
        );
        const { rerender } = render(list(0, false));
        const wrapper = document.querySelector<HTMLElement>(".git-virtual-rows")!;
        expect(wrapper.style.height).toBe(`${300 * 26}px`);
        expect(screen.getByText("row 0")).toBeInTheDocument();
        expect(screen.queryByText("row 299")).toBeNull();

        rerender(list(250, false));
        const tops = () => scrollTo.mock.calls.map(([options]) => (options as ScrollToOptions).top ?? 0);
        expect(Math.max(0, ...tops())).toBeLessThan(250 * 26 - 260);
        rerender(list(250, true));
        await waitFor(() => expect(Math.max(...tops())).toBeGreaterThanOrEqual(250 * 26 - 260));
    });
});
