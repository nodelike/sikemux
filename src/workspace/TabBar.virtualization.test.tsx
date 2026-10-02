import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { TabBar } from "./TabBar";

const { scrollToIndex } = vi.hoisted(() => ({ scrollToIndex: vi.fn() }));
vi.mock("@tanstack/react-virtual", () => ({
    useVirtualizer: ({ count }: { count: number }) => ({
        getVirtualItems: () =>
            Array.from({ length: Math.min(count, 12) }, (_, index) => ({ index, key: index, start: index * 160, size: 160, end: (index + 1) * 160 })),
        getTotalSize: () => count * 160,
        measureElement: vi.fn(),
        scrollToIndex,
    }),
}));

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

const strip = (count: number, activeIndex: number) =>
    Array.from({ length: count }, (_, index) => ({ id: `tab-${index}`, label: `Tab ${index}`, active: index === activeIndex }));

it("mounts a bounded window for a large tab strip and navigates by full-list index", () => {
    const onSelect = vi.fn();
    render(<TabBar variant="agent" tabs={strip(100, 0)} onSelect={onSelect} />);

    expect(screen.getAllByRole("tab")).toHaveLength(12);
    fireEvent.keyDown(screen.getByRole("tab", { name: "Tab 0" }), { key: "End" });
    expect(onSelect).toHaveBeenCalledWith("tab-99");
    expect(scrollToIndex).toHaveBeenLastCalledWith(99, { align: "auto" });
});

/**
 * jsdom lays nothing out, so the strip is told where it and its pills are: one
 * screen of strip with the last pill sitting off the end of it.
 */
function laidOut(container: HTMLElement, pillWidth = 160): { strip: HTMLElement; scrollBy: ReturnType<typeof vi.fn> } {
    const strip = container.querySelector(".tabbar-tabs") as HTMLElement;
    const scrollBy = vi.fn();
    strip.scrollBy = scrollBy;
    strip.getBoundingClientRect = () => new DOMRect(0, 0, 400, 30);
    for (const [index, tab] of [...container.querySelectorAll('[role="tab"]')].entries()) {
        const left = index * pillWidth;
        (tab as HTMLElement).getBoundingClientRect = () => new DOMRect(left, 0, pillWidth, 30);
    }
    return { strip, scrollBy };
}

/*
 * A strip too narrow for its tabs used to leave the active pill off the end of
 * itself unless it was long enough to be virtualized, so the tab you were on
 * could be somewhere you could not see.
 */
it("brings the active pill into view whichever tab becomes active", () => {
    const { container, rerender } = render(<TabBar variant="agent" tabs={strip(3, 0)} onSelect={vi.fn()} />);
    const { scrollBy } = laidOut(container);

    rerender(<TabBar variant="agent" tabs={strip(3, 2)} onSelect={vi.fn()} />);

    expect(scrollBy).toHaveBeenCalledWith({ left: 80, behavior: "smooth" });
});

/*
 * And it may only ever scroll the strip. `scrollIntoView` scrolls every
 * scrollable ancestor too, and the stage under the strip is one of them: the
 * room it took out of the stage for a pill at the end left the tabs and the
 * window below them parked to one side, with a band of shell down the edge.
 */
it("leaves the stage the strip sits on where it is", () => {
    const scrollIntoView = vi.spyOn(Element.prototype, "scrollIntoView");
    const { container, rerender } = render(<TabBar variant="agent" tabs={strip(3, 0)} onSelect={vi.fn()} />);
    laidOut(container);

    rerender(<TabBar variant="agent" tabs={strip(3, 2)} onSelect={vi.fn()} />);
    fireEvent.click(screen.getByRole("tab", { name: "Tab 2" }));

    expect(scrollIntoView).not.toHaveBeenCalled();
});

it("leaves a pill already in view alone", () => {
    const { container, rerender } = render(<TabBar variant="agent" tabs={strip(3, 2)} onSelect={vi.fn()} />);
    const { scrollBy } = laidOut(container, 100);

    rerender(<TabBar variant="agent" tabs={strip(3, 1)} onSelect={vi.fn()} />);

    expect(scrollBy).not.toHaveBeenCalled();
});

it("brings it into view on a virtualized strip too", () => {
    const { container, rerender } = render(<TabBar variant="agent" tabs={strip(100, 0)} onSelect={vi.fn()} />);
    const { scrollBy } = laidOut(container);

    rerender(<TabBar variant="agent" tabs={strip(100, 5)} onSelect={vi.fn()} />);

    expect(scrollBy).toHaveBeenCalledWith({ left: 560, behavior: "smooth" });
});

it("jumps rather than glides when motion is reduced", () => {
    vi.stubGlobal("matchMedia", (query: string) => ({ matches: query === "(prefers-reduced-motion: reduce)", media: query }));
    const { container, rerender } = render(<TabBar variant="agent" tabs={strip(3, 0)} onSelect={vi.fn()} />);
    const { scrollBy } = laidOut(container);

    rerender(<TabBar variant="agent" tabs={strip(3, 2)} onSelect={vi.fn()} />);

    expect(scrollBy).toHaveBeenCalledWith({ left: 80, behavior: "auto" });
    vi.unstubAllGlobals();
});
