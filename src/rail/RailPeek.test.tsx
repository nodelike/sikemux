import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { RailPeek } from "./RailPeek";

afterEach(() => {
    cleanup();
    vi.useRealTimers();
});

describe("RailPeek", () => {
    it("mounts the rail on edge hover and removes it after the exit animation", () => {
        vi.useFakeTimers();
        render(
            <RailPeek edge="start">
                <aside>sessions</aside>
            </RailPeek>,
        );

        const peek = screen.getByTestId("rail-peek-start");
        expect(screen.queryByText("sessions")).not.toBeInTheDocument();

        fireEvent.pointerEnter(peek);
        expect(screen.getByText("sessions")).toBeInTheDocument();
        expect(screen.getByText("sessions").parentElement).toHaveClass("rail-peek-panel--open");

        fireEvent.pointerLeave(peek);
        expect(screen.getByText("sessions").parentElement).toHaveClass("rail-peek-panel--closing");

        act(() => vi.advanceTimersByTime(180));
        expect(screen.queryByText("sessions")).not.toBeInTheDocument();
    });

    it("cancels a pending close when the pointer returns", () => {
        vi.useFakeTimers();
        render(
            <RailPeek edge="end">
                <aside>agents</aside>
            </RailPeek>,
        );

        const peek = screen.getByTestId("rail-peek-end");
        fireEvent.pointerEnter(peek);
        fireEvent.pointerLeave(peek);
        fireEvent.pointerEnter(peek);
        act(() => vi.advanceTimersByTime(180));

        expect(screen.getByText("agents").parentElement).toHaveClass("rail-peek-panel--open");
    });

    it("stays open while focus remains inside the rail", () => {
        vi.useFakeTimers();
        render(
            <RailPeek edge="end">
                <button>search agents</button>
            </RailPeek>,
        );

        const peek = screen.getByTestId("rail-peek-end");
        fireEvent.pointerEnter(peek);
        const search = screen.getByRole("button", { name: "search agents" });
        search.focus();
        fireEvent.pointerLeave(peek);
        act(() => vi.advanceTimersByTime(180));

        expect(search).toBeInTheDocument();

        fireEvent.blur(search, { relatedTarget: document.body });
        act(() => vi.advanceTimersByTime(180));
        expect(screen.queryByRole("button", { name: "search agents" })).not.toBeInTheDocument();
    });

    it("opens a little way in from the edge, not only on the gap", () => {
        vi.useFakeTimers();
        render(
            <RailPeek edge="start">
                <aside>sessions</aside>
            </RailPeek>,
        );

        const peek = screen.getByTestId("rail-peek-start");
        vi.spyOn(peek, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 250, 600));

        fireEvent.pointerMove(window, { clientX: 20, clientY: 300 });
        expect(screen.getByText("sessions")).toBeInTheDocument();

        fireEvent.pointerMove(document.body, { clientX: 400, clientY: 300 });
        act(() => vi.advanceTimersByTime(180));
        expect(screen.queryByText("sessions")).not.toBeInTheDocument();

        fireEvent.pointerMove(window, { clientX: 20, clientY: 300, buttons: 1 });
        expect(screen.queryByText("sessions")).not.toBeInTheDocument();
    });
});
