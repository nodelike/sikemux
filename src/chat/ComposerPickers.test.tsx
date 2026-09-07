import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ComposerPickers, sessionConfigs } from "./ComposerPickers";
import { getState, setState } from "../state/store";

const mocks = vi.hoisted(() => ({ addAgent: vi.fn() }));
vi.mock("../state/commands", () => ({ addAgent: mocks.addAgent }));
const initial = getState();
afterEach(() => {
    cleanup();
    setState(initial, true);
    vi.clearAllMocks();
});

describe("composer pickers", () => {
    it("flattens provider model groups without changing their identifiers", () => {
        expect(
            sessionConfigs({
                configOptions: [
                    {
                        id: "model",
                        type: "select",
                        currentValue: "custom/model",
                        options: [{ group: "custom", name: "Custom provider", options: [{ value: "custom/model", name: "My model" }] }],
                    },
                ],
            })[0].options,
        ).toEqual([{ value: "custom/model", label: "My model", description: "Custom provider" }]);
    });

    it("starts a new chat with the selected configured agent", () => {
        setState({ providerProfiles: [{ id: "work", name: "Work Claude", provider: "claude", accent: "#fff" }] });
        render(
            <ComposerPickers
                agent={{ id: "a", type: "codex", title: "Codex", startup: "codex" }}
                cwd="/repo"
                setup={{}}
                disabled={false}
                onConfig={() => {}}
            />,
        );
        fireEvent.click(screen.getByRole("button", { name: "Agent" }));
        fireEvent.click(screen.getByRole("option", { name: /Work Claude/ }));
        expect(mocks.addAgent).toHaveBeenCalledWith("claude", undefined, undefined, expect.objectContaining({ profileId: "work", cwd: "/repo" }));
    });

    it("locks the agent after messages while keeping model and effort available", () => {
        render(
            <ComposerPickers
                agent={{ id: "a", type: "codex", title: "Codex", startup: "codex" }}
                cwd="/repo"
                disabled={false}
                agentLocked
                onConfig={() => {}}
                setup={{
                    configOptions: [
                        { id: "model", type: "select", currentValue: "model", options: [{ value: "model", name: "My model" }] },
                        { id: "reasoning_effort", type: "select", currentValue: "high", options: [{ value: "high", name: "High" }] },
                    ],
                }}
            />,
        );
        expect(screen.getByRole("button", { name: "Agent" })).toBeDisabled();
        expect(screen.getByRole("button", { name: "Model" })).toBeEnabled();
        expect(screen.getByRole("button", { name: "Reasoning effort" })).toBeEnabled();
    });

    it("closes the model menu with Escape and returns focus to its trigger", () => {
        render(
            <ComposerPickers
                agent={{ id: "a", type: "codex", title: "Codex", startup: "codex" }}
                cwd="/repo"
                disabled={false}
                onConfig={() => {}}
                setup={{ configOptions: [{ id: "model", type: "select", currentValue: "model", options: [{ value: "model", name: "My model" }] }] }}
            />,
        );
        const trigger = screen.getByRole("button", { name: "Model" });
        fireEvent.click(trigger);
        fireEvent.keyDown(screen.getByRole("combobox", { name: "Search model" }), { key: "Escape" });
        expect(trigger).toHaveFocus();
        expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    });
});
