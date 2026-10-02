import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { FoldPanel } from "./FoldPanel";

afterEach(cleanup);

it("shows its count and a summary while folded, and its content with a resize edge while open", () => {
    const onToggle = vi.fn();
    const onResize = vi.fn();
    const panel = (open: boolean, height: number | null = null) => (
        <FoldPanel
            label="Commits"
            count={3}
            summary="fix: latest"
            open={open}
            height={height}
            onToggle={onToggle}
            onResize={onResize}
            badge={<span>badge</span>}>
            <div>commit list</div>
        </FoldPanel>
    );
    const { rerender, container } = render(panel(false));
    const toggle = screen.getByRole("button", { name: /Commits/ });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(toggle).toHaveTextContent("3");
    expect(screen.getByText("fix: latest")).toBeInTheDocument();
    expect(screen.getByText("badge")).toBeInTheDocument();
    expect(screen.queryByText("commit list")).toBeNull();
    expect(screen.queryByRole("separator")).toBeNull();
    fireEvent.click(toggle);
    expect(onToggle).toHaveBeenCalledOnce();

    rerender(panel(true, 240));
    expect(screen.getByText("commit list")).toBeInTheDocument();
    expect(screen.queryByText("fix: latest")).toBeNull();
    expect(container.querySelector<HTMLElement>(".git-history")!.style.flex).toBe("0 0 240px");
    fireEvent.doubleClick(screen.getByRole("separator", { name: "Resize commits" }));
    expect(onResize).toHaveBeenCalledWith(null);
});

it("hides a zero count", () => {
    render(
        <FoldPanel label="Commits" count={0} open={false} height={null} onToggle={() => {}} onResize={() => {}}>
            <div />
        </FoldPanel>,
    );
    expect(screen.getByRole("button", { name: "Commits" })).not.toHaveTextContent("0");
});
