import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { AgentPresentationState } from "../state/types";
import { AgentStateIndicator } from "./AgentStateIndicator";

afterEach(cleanup);

describe("AgentStateIndicator", () => {
    it("renders working as a 3×3 grid of twinkling cells", () => {
        const { container } = render(<AgentStateIndicator state="working" />);
        expect(screen.getByRole("img", { name: "Working" })).toBeInTheDocument();
        expect(container.querySelectorAll(".agent-state-loader i")).toHaveLength(9);
        expect(container.querySelector("svg")).not.toBeInTheDocument();
    });

    it.each([
        ["done", "Done — unseen"],
        ["blocked", "Needs input"],
    ] as const)("renders %s as a dot", (state, label) => {
        const { container } = render(<AgentStateIndicator state={state} />);
        expect(screen.getByRole("img", { name: label })).toBeInTheDocument();
        expect(container.querySelector(".agent-state-dot")).toBeInTheDocument();
        expect(container.querySelector(".agent-state-loader")).not.toBeInTheDocument();
    });

    it.each(["idle", "stopped", "unknown"] as const)("renders nothing for %s", (state) => {
        const { container } = render(<AgentStateIndicator state={state as AgentPresentationState} />);
        expect(container).toBeEmptyDOMElement();
    });

    it("marks a settled agent that still has shells or monitors running", () => {
        const { container } = render(<AgentStateIndicator state="idle" background />);
        expect(screen.getByRole("img", { name: "Shells or monitors still running" })).toBeInTheDocument();
        expect(container.querySelector(".state-background")).toBeInTheDocument();
        expect(container.querySelector(".agent-state-icon")).toBeInTheDocument();
        expect(container.querySelector(".agent-state-dot")).not.toBeInTheDocument();
    });

    it("keeps the spinner while working, background work or not", () => {
        const { container } = render(<AgentStateIndicator state="working" background />);
        expect(container.querySelector(".agent-state-loader")).toBeInTheDocument();
    });

    it("prefers needs-input over background work", () => {
        render(<AgentStateIndicator state="blocked" background />);
        expect(screen.getByRole("img", { name: "Needs input" })).toBeInTheDocument();
    });
});
