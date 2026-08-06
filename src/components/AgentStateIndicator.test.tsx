import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { AgentPresentationState } from "../state/types";
import { AgentStateIndicator } from "./AgentStateIndicator";

describe("AgentStateIndicator", () => {
    it("renders working as a dedicated circular CSS loader", () => {
        const { container } = render(<AgentStateIndicator state="working" />);
        expect(screen.getByRole("img", { name: "Working" })).toBeInTheDocument();
        expect(container.querySelector(".agent-state-loader")).toBeInTheDocument();
        expect(container.querySelector("svg")).not.toBeInTheDocument();
    });

    it.each([
        ["blocked", "Needs input"],
        ["done", "Done — unseen"],
        ["idle", "Idle"],
        ["unknown", "Unknown"],
    ] as const)("renders the %s state as an accessible SVG icon", (state, label) => {
        const { container } = render(<AgentStateIndicator state={state as AgentPresentationState} />);
        expect(screen.getByRole("img", { name: label })).toBeInTheDocument();
        expect(container.querySelector("svg.agent-state-icon")).toBeInTheDocument();
        expect(container).not.toHaveTextContent(/[↻!✓○?]/);
    });
});
