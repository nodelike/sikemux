import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AgentPalettePortal } from "./AgentPalettePortal";

vi.mock("./AgentPalette", () => ({
    AgentPalette: () => <div data-testid="agent-picker" />,
}));

afterEach(cleanup);

describe("AgentPalettePortal", () => {
    it("renders the picker inside the center stage", () => {
        render(
            <>
                <aside data-testid="project-rail" />
                <aside data-testid="agent-rail" />
                <main className="stage" data-testid="stage" />
                <AgentPalettePortal />
            </>,
        );

        const stage = screen.getByTestId("stage");
        expect(within(stage).getByTestId("agent-picker")).toBeInTheDocument();
        expect(screen.getByTestId("project-rail")).not.toContainElement(screen.getByTestId("agent-picker"));
        expect(screen.getByTestId("agent-rail")).not.toContainElement(screen.getByTestId("agent-picker"));
    });
});
