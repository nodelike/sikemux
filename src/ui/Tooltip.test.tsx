import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Tooltip } from "./Tooltip";

afterEach(cleanup);

describe("Tooltip", () => {
    it("opens immediately on focus and closes on blur", async () => {
        render(
            <Tooltip label="Toggle sessions rail">
                <button type="button">rail</button>
            </Tooltip>,
        );

        const trigger = screen.getByRole("button", { name: "rail" });
        fireEvent.focus(trigger);
        const tooltip = await screen.findByRole("tooltip");
        expect(tooltip).toHaveTextContent("Toggle sessions rail");
        expect(trigger).toHaveAttribute("aria-describedby", tooltip.id);

        fireEvent.blur(trigger);
        await waitFor(() => expect(screen.queryByRole("tooltip")).not.toBeInTheDocument());
    });

    it("closes on Escape", async () => {
        render(
            <Tooltip label="Full path">
                <button type="button">File</button>
            </Tooltip>,
        );
        fireEvent.focus(screen.getByRole("button"));
        await screen.findByRole("tooltip");
        fireEvent.keyDown(window, { key: "Escape" });
        expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
    });

    it("waits before opening on hover so a passing cursor does not flash it", async () => {
        vi.useFakeTimers();
        try {
            render(
                <Tooltip label="Focus mode">
                    <button type="button">zen</button>
                </Tooltip>,
            );

            fireEvent.mouseEnter(screen.getByRole("button", { name: "zen" }));
            expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();

            act(() => vi.advanceTimersByTime(400));
            expect(screen.getByRole("tooltip")).toHaveTextContent("Focus mode");
        } finally {
            vi.useRealTimers();
        }
    });

    it("keeps the child's own handlers and renders it untouched without a label", () => {
        const onClick = vi.fn();
        const onMouseEnter = vi.fn();
        render(
            <Tooltip label="">
                <button type="button" onClick={onClick} onMouseEnter={onMouseEnter}>
                    bare
                </button>
            </Tooltip>,
        );

        const trigger = screen.getByRole("button", { name: "bare" });
        fireEvent.mouseEnter(trigger);
        fireEvent.click(trigger);
        expect(onMouseEnter).toHaveBeenCalledOnce();
        expect(onClick).toHaveBeenCalledOnce();
        expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
    });
});
