import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import { ContextMeter } from "./ContextMeter";

afterEach(cleanup);

it("draws nothing until the agent reports its context window", () => {
    const { container } = render(<ContextMeter usage={null} agent="claude" />);
    expect(container).toBeEmptyDOMElement();
});

it("fills the ring and explains it when focused", () => {
    render(<ContextMeter usage={{ used: 84_200, size: 200_000, cost: { amount: 1.254, currency: "USD" } }} agent="claude" />);
    const meter = screen.getByRole("img", { name: "Context window 42% used" });
    expect(meter).toHaveAttribute("data-tone", "steady");
    expect(meter.querySelector(".chat-context-fill")).toHaveAttribute("stroke-dasharray", "42.1 100");

    act(() => fireEvent.focus(meter));
    const tip = screen.getByRole("tooltip");
    expect(tip).toHaveTextContent("42% used · 84.2K of 200K tokens");
    expect(tip).toHaveTextContent("Session cost $1.25");
});

it("turns hot near the end of the window and leaves cost out when none is reported", () => {
    render(<ContextMeter usage={{ used: 950_000, size: 1_000_000 }} agent="codex" />);
    const meter = screen.getByRole("img", { name: "Context window 95% used" });
    expect(meter).toHaveAttribute("data-tone", "hot");

    act(() => fireEvent.focus(meter));
    expect(screen.getByRole("tooltip")).toHaveTextContent("950K of 1M tokens");
    expect(screen.getByRole("tooltip")).not.toHaveTextContent("cost");
});
