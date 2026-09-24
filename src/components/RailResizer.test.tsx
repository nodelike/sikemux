import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { RailResizer } from "./RailResizer";
import { getState, setState } from "../state/store";

const initial = getState();

afterEach(() => {
    cleanup();
    setState(initial, true);
});

describe("RailResizer", () => {
    it("widens the side rail when its handle moves right", () => {
        setState({ sideRailWidth: 258 });
        render(<RailResizer edge="start" />);
        fireEvent.keyDown(screen.getByRole("separator"), { key: "ArrowRight" });
        expect(getState().sideRailWidth).toBe(274);
    });

    it("widens the agent rail when its handle moves left", () => {
        setState({ agentRailWidth: 288 });
        render(<RailResizer edge="end" />);
        fireEvent.keyDown(screen.getByRole("separator"), { key: "ArrowLeft", shiftKey: true });
        expect(getState().agentRailWidth).toBe(328);
    });

    it("stops at the rail's bounds", () => {
        setState({ sideRailWidth: 190 });
        render(<RailResizer edge="start" />);
        fireEvent.keyDown(screen.getByRole("separator"), { key: "ArrowLeft", shiftKey: true });
        expect(getState().sideRailWidth).toBe(180);
    });
});
