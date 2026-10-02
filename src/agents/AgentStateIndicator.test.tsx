import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { AgentPresentationState } from "../state/types";
import { AgentStateIndicator } from "./AgentStateIndicator";

afterEach(cleanup);

describe("AgentStateIndicator", () => {
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

    it("prefers needs-input over background work", () => {
        render(<AgentStateIndicator state="blocked" background />);
        expect(screen.getByRole("img", { name: "Needs input" })).toBeInTheDocument();
    });
});
