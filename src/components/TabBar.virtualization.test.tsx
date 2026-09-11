import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
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

it("mounts a bounded window for a large tab strip and navigates by full-list index", () => {
    const onSelect = vi.fn();
    render(
        <TabBar
            variant="agent"
            tabs={Array.from({ length: 100 }, (_, index) => ({ id: `tab-${index}`, label: `Tab ${index}`, active: index === 0 }))}
            onSelect={onSelect}
        />,
    );

    expect(screen.getAllByRole("tab")).toHaveLength(12);
    fireEvent.keyDown(screen.getByRole("tab", { name: "Tab 0" }), { key: "End" });
    expect(onSelect).toHaveBeenCalledWith("tab-99");
    expect(scrollToIndex).toHaveBeenLastCalledWith(99, { align: "auto" });
});
