import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { AgentStateIndicator } from "./AgentStateIndicator";

afterEach(cleanup);

describe("AgentStateIndicator", () => {
    it.each([
        ["done", "Done"],
        ["blocked", "Needs input"],
    ] as const)("renders %s as a dot", (state, label) => {
        const { container } = render(<AgentStateIndicator display={{ group: state, label }} />);
        expect(screen.getByRole("img", { name: label })).toBeInTheDocument();
        expect(container.querySelector(".agent-state-dot")).toBeInTheDocument();
        expect(container.querySelector(".agent-state-loader")).not.toBeInTheDocument();
    });

    it("renders nothing for idle", () => {
        const { container } = render(<AgentStateIndicator display={{ group: "idle", label: "Idle" }} />);
        expect(container).toBeEmptyDOMElement();
    });

    it("prefers needs-input over background work", () => {
        render(<AgentStateIndicator display={{ group: "blocked", label: "Needs input", subagents: 0, tasks: 1 } as never} />);
        expect(screen.getByRole("img", { name: "Needs input" })).toBeInTheDocument();
    });
});
