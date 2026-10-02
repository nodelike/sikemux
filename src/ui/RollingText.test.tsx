import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { CountUp, RollingText } from "./RollingText";

afterEach(() => {
    cleanup();
    delete (HTMLElement.prototype as { animate?: unknown }).animate;
    vi.unstubAllGlobals();
});

it("rolls only the characters that changed, and nothing on mount", () => {
    const animate = vi.fn(() => ({ finished: Promise.resolve() }) as unknown as Animation);
    Object.defineProperty(HTMLElement.prototype, "animate", { value: animate, configurable: true, writable: true });
    vi.stubGlobal("matchMedia", () => ({ matches: false }) as MediaQueryList);
    const { container, rerender } = render(<RollingText text="9:41" />);
    expect(animate).not.toHaveBeenCalled();
    rerender(<RollingText text="9:42" />);
    expect(animate).toHaveBeenCalledTimes(1);
    expect(animate.mock.contexts[0]).toBe(container.querySelectorAll(".rolling-text > span")[3]);
    expect(container.textContent).toBe("9:42");
});

it("shows a new count at once where nothing can animate", () => {
    const { rerender } = render(
        <span data-testid="n">
            <CountUp value={34} />
        </span>,
    );
    rerender(
        <span data-testid="n">
            <CountUp value={41} />
        </span>,
    );
    expect(screen.getByTestId("n").textContent).toBe("41");
});
