import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TabBar, type TabDescriptor } from "./TabBar";
import { TAB_SLIDE_MS } from "./tabDrag";

const tabs: TabDescriptor[] = ["a", "b", "c"].map((id) => ({ id, label: id }));
const pill = (name: string) => screen.getByRole("tab", { name }).closest<HTMLElement>(".tab-wrap")!;

/* jsdom lays nothing out, so each pill reports where it would sit: 100px wide, side by side. */
function placePills() {
    document.querySelectorAll<HTMLElement>(".tab-wrap").forEach((wrap) => {
        const left = Number(wrap.dataset.index) * 100;
        vi.spyOn(wrap, "getBoundingClientRect").mockReturnValue({ left, right: left + 100, top: 0, bottom: 30, width: 100, height: 30 } as DOMRect);
    });
}

function renderStrip(props: Partial<Parameters<typeof TabBar>[0]> = {}) {
    const onSelect = vi.fn();
    const onReorder = vi.fn();
    render(<TabBar variant="agent" tabs={tabs} onSelect={onSelect} onReorder={onReorder} {...props} />);
    placePills();
    return { onSelect, onReorder, tab: (name: string) => screen.getByRole("tab", { name }) };
}

describe("dragging a tab to a new place", () => {
    beforeEach(() => vi.useFakeTimers());
    afterEach(() => {
        cleanup();
        vi.useRealTimers();
        vi.restoreAllMocks();
        document.body.classList.remove("is-sorting-tabs");
    });

    it("lifts the tab, follows the pointer and slides its neighbours aside", () => {
        const { tab } = renderStrip();

        fireEvent.pointerDown(tab("a"), { button: 0, clientX: 10, clientY: 10 });
        fireEvent.pointerMove(window, { clientX: 220, clientY: 10 });

        expect(pill("a")).toHaveClass("tab-lifted");
        // Held at the strip's far edge: it may not travel past c's right side.
        expect(pill("a").style.transform).toBe("translateX(200px)");
        expect(pill("b").style.transform).toBe("translateX(-100px)");
        expect(pill("c").style.transform).toBe("translateX(-100px)");
        expect(pill("a").closest(".tabbar")).toHaveClass("is-reordering");
    });

    it("glides into its slot on release, then commits the new order", () => {
        const { onReorder, tab } = renderStrip();

        fireEvent.pointerDown(tab("a"), { button: 0, clientX: 10, clientY: 10 });
        fireEvent.pointerMove(window, { clientX: 220, clientY: 10 });
        fireEvent.pointerUp(window, { clientX: 220, clientY: 10 });

        expect(pill("a").style.transform).toBe("translateX(200px)");
        expect(onReorder).not.toHaveBeenCalled();

        act(() => vi.advanceTimersByTime(TAB_SLIDE_MS));

        expect(onReorder).toHaveBeenCalledWith("a", "c", "after");
        expect(pill("a").style.transform).toBe("");
        expect(pill("b").style.transform).toBe("");
        expect(pill("a")).not.toHaveClass("tab-lifted");
    });

    it("leaves the neighbours alone until the tab crosses one's middle", () => {
        const { tab } = renderStrip();

        fireEvent.pointerDown(tab("a"), { button: 0, clientX: 10, clientY: 10 });
        fireEvent.pointerMove(window, { clientX: 50, clientY: 10 });

        expect(pill("a").style.transform).toBe("translateX(40px)");
        expect(pill("b").style.transform).toBe("");
    });

    it("does not select the tab when the drag's release lands as a click", () => {
        const { onSelect, tab } = renderStrip();

        fireEvent.pointerDown(tab("a"), { button: 0, clientX: 10, clientY: 10 });
        fireEvent.pointerMove(window, { clientX: 220, clientY: 10 });
        fireEvent.pointerUp(window, { clientX: 220, clientY: 10 });
        fireEvent.click(tab("a"));

        expect(onSelect).not.toHaveBeenCalled();
    });

    it("treats a press that barely moves as an ordinary click", () => {
        const { onSelect, onReorder, tab } = renderStrip();

        fireEvent.pointerDown(tab("b"), { button: 0, clientX: 150, clientY: 10 });
        fireEvent.pointerMove(window, { clientX: 152, clientY: 11 });
        fireEvent.pointerUp(window, { clientX: 152, clientY: 11 });
        fireEvent.click(tab("b"));
        act(() => vi.advanceTimersByTime(TAB_SLIDE_MS));

        expect(onReorder).not.toHaveBeenCalled();
        expect(onSelect).toHaveBeenCalledWith("b");
    });

    it("puts everything back on Escape", () => {
        const { onReorder, tab } = renderStrip();

        fireEvent.pointerDown(tab("a"), { button: 0, clientX: 10, clientY: 10 });
        fireEvent.pointerMove(window, { clientX: 220, clientY: 10 });
        fireEvent.keyDown(window, { key: "Escape" });

        expect(pill("b").style.transform).toBe("");
        act(() => vi.advanceTimersByTime(TAB_SLIDE_MS));

        expect(onReorder).not.toHaveBeenCalled();
        expect(pill("a").style.transform).toBe("");
        expect(document.body).not.toHaveClass("is-sorting-tabs");
    });

    it("keeps a strip without a reorder handler fixed", () => {
        const { tab } = renderStrip({ onReorder: undefined });

        fireEvent.pointerDown(tab("a"), { button: 0, clientX: 10, clientY: 10 });
        fireEvent.pointerMove(window, { clientX: 220, clientY: 10 });

        expect(pill("a")).not.toHaveClass("tab-lifted");
        expect(pill("a").style.transform).toBe("");
    });

    it("opens no gap where the owner rules the drop out", () => {
        const { onReorder, tab } = renderStrip({ canReorder: () => false });

        fireEvent.pointerDown(tab("a"), { button: 0, clientX: 10, clientY: 10 });
        fireEvent.pointerMove(window, { clientX: 220, clientY: 10 });

        expect(pill("b").style.transform).toBe("");
        fireEvent.pointerUp(window, { clientX: 220, clientY: 10 });
        act(() => vi.advanceTimersByTime(TAB_SLIDE_MS));
        expect(onReorder).not.toHaveBeenCalled();
    });

    it("skips the slide when reduced motion is asked for", () => {
        vi.stubGlobal("matchMedia", (query: string) => ({ matches: query.includes("reduce"), media: query }));
        const { onReorder, tab } = renderStrip();

        fireEvent.pointerDown(tab("a"), { button: 0, clientX: 10, clientY: 10 });
        fireEvent.pointerMove(window, { clientX: 220, clientY: 10 });
        fireEvent.pointerUp(window, { clientX: 220, clientY: 10 });

        expect(onReorder).toHaveBeenCalledWith("a", "c", "after");
        vi.unstubAllGlobals();
    });
});
