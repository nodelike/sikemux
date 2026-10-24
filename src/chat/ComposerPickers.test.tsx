import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ComposerPickers } from "./ComposerPickers";
import type { ConfigChoice } from "../api/generated/wire";
import { getState, setState } from "../state/store";

const mocks = vi.hoisted(() => ({ onAgent: vi.fn() }));
const codexAgent = { id: "a", type: "codex" as const, title: "Codex", startup: "codex" };

function picker(current: string, choices: ConfigChoice[], id = "model") {
    return { id, current, choices };
}

const noEffort = picker("", [], "effort");
const threeModels = picker("sonnet", [
    { value: "sonnet", label: "Sonnet" },
    { value: "opus", label: "Opus" },
    { value: "haiku", label: "Haiku", description: "Fastest" },
]);
const initial = getState();
afterEach(() => {
    cleanup();
    setState(initial, true);
    vi.clearAllMocks();
});

describe("composer pickers", () => {
    it("selects the harness for the existing empty chat", () => {
        setState({ providerProfiles: [{ id: "work", name: "Work Claude", provider: "claude", accent: "#fff" }] });
        render(<ComposerPickers agent={{ id: "a", type: "codex", title: "Codex" }} onAgent={mocks.onAgent} disabled={false} onConfig={() => {}} />);
        fireEvent.click(screen.getByRole("button", { name: "Model" }));
        fireEvent.click(screen.getByRole("button", { name: "Claude" }));
        expect(mocks.onAgent).toHaveBeenCalledWith("claude", "work");
    });

    it("keeps the menu open through an agent switch and fills in the new agent's models", () => {
        const codex = { id: "a", type: "codex" as const, title: "Codex", startup: "codex" };
        const claude = { ...codex, type: "claude" as const, title: "Claude" };
        const models = (value: string, label: string) => picker(value, [{ value, label }]);
        const codexModel = models("astra", "GPT-6 Astra");
        const props = { onAgent: mocks.onAgent, onConfig: () => {} };
        const { rerender } = render(<ComposerPickers {...props} agent={codex} model={codexModel} disabled={false} />);
        fireEvent.click(screen.getByRole("button", { name: "Model" }));
        fireEvent.click(screen.getByRole("button", { name: /Claude/ }));
        expect(mocks.onAgent).toHaveBeenCalledWith("claude", expect.anything());
        rerender(<ComposerPickers {...props} agent={claude} model={codexModel} disabled={false} />);
        expect(screen.getByText("Loading models…")).toBeInTheDocument();
        rerender(<ComposerPickers {...props} agent={claude} disabled />);
        expect(screen.getByText("Loading models…")).toBeInTheDocument();
        rerender(<ComposerPickers {...props} agent={claude} model={models("opus", "Opus")} disabled={false} />);
        expect(screen.queryByText("Loading models…")).not.toBeInTheDocument();
        expect(screen.getByRole("option", { name: /Opus/ })).toBeInTheDocument();
        expect(screen.getByRole("button", { name: /Claude/ })).toHaveAttribute("aria-pressed", "true");
    });

    it("offers no effort picker to an agent whose model has none", () => {
        render(
            <ComposerPickers
                agent={{ id: "a", type: "hermes", title: "Hermes" }}
                onAgent={mocks.onAgent}
                disabled={false}
                onConfig={() => {}}
                model={picker("m", [{ value: "m", label: "M" }])}
            />,
        );
        expect(screen.getByRole("button", { name: "Model" })).toBeInTheDocument();
        expect(screen.queryByRole("button", { name: "Reasoning effort" })).not.toBeInTheDocument();
    });

    it("lists a harness once when its built-in profile is the default", () => {
        render(<ComposerPickers agent={{ id: "a", type: "codex", title: "Codex" }} onAgent={mocks.onAgent} disabled={false} onConfig={() => {}} />);
        fireEvent.click(screen.getByRole("button", { name: "Model" }));
        const agents = screen.getByRole("group", { name: "Agent" }).querySelectorAll("button");
        expect(Array.from(agents, (button) => button.textContent)).toEqual(["Claude", "Codex"]);
        expect(screen.getByRole("button", { name: /Codex/ })).toHaveAttribute("aria-pressed", "true");
    });

    it("drops the agent picker after messages while keeping model and effort available", () => {
        render(
            <ComposerPickers
                agent={{ id: "a", type: "codex", title: "Codex" }}
                onAgent={mocks.onAgent}
                disabled={false}
                agentLocked
                onConfig={() => {}}
                model={picker("model", [{ value: "model", label: "My model" }])}
                effort={picker("high", [{ value: "high", label: "High" }], "reasoning_effort")}
            />,
        );
        expect(screen.getByRole("button", { name: "Model" }).querySelector(".agent-glyph.codex")).not.toBeNull();
        expect(screen.getByRole("button", { name: "Model" })).toBeEnabled();
        fireEvent.click(screen.getByRole("button", { name: "Model" }));
        expect(screen.queryByRole("group", { name: "Agent" })).not.toBeInTheDocument();
        expect(screen.getByRole("button", { name: "Reasoning effort" })).toBeEnabled();
    });

    it("picks a model when the click blurs the search without focusing anything", () => {
        const onConfig = vi.fn();
        render(
            <ComposerPickers
                agent={{ id: "a", type: "codex", title: "Codex" }}
                onAgent={mocks.onAgent}
                disabled={false}
                onConfig={onConfig}
                model={picker("sonnet", [
                    { value: "sonnet", label: "Sonnet 5", description: "Sonnet 5 · Efficient for routine tasks" },
                    { value: "opus", label: "Opus" },
                ])}
            />,
        );
        fireEvent.click(screen.getByRole("button", { name: "Model" }));
        expect(screen.queryByText(/Efficient for routine tasks/)).not.toBeInTheDocument();
        expect(screen.getByRole("option", { name: /Sonnet 5/ }).querySelector(".agent-glyph.codex")).not.toBeNull();
        const option = screen.getByRole("option", { name: /Opus/ });
        fireEvent.focusOut(screen.getByRole("combobox", { name: "Search model" }), { relatedTarget: null });
        fireEvent.click(option);
        expect(onConfig).toHaveBeenCalledWith(expect.objectContaining({ id: "model" }), "opus");
    });

    it("closes the model menu with Escape and returns focus to its trigger", () => {
        render(
            <ComposerPickers
                agent={{ id: "a", type: "codex", title: "Codex" }}
                onAgent={mocks.onAgent}
                disabled={false}
                onConfig={() => {}}
                model={picker("model", [{ value: "model", label: "My model" }])}
            />,
        );
        const trigger = screen.getByRole("button", { name: "Model" });
        fireEvent.click(trigger);
        fireEvent.keyDown(screen.getByRole("combobox", { name: "Search model" }), { key: "Escape" });
        expect(trigger).toHaveFocus();
        expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    });

    it("moves through models with the arrow keys, wrapping at each end, and picks one with Enter", () => {
        const onConfig = vi.fn();
        render(<ComposerPickers agent={codexAgent} onAgent={mocks.onAgent} disabled={false} onConfig={onConfig} model={threeModels} />);
        fireEvent.click(screen.getByRole("button", { name: "Model" }));
        const search = screen.getByRole("combobox", { name: "Search model" });
        expect(search).toHaveFocus();

        fireEvent.keyDown(search, { key: "ArrowUp" });
        expect(screen.getByRole("option", { name: /Haiku/ })).toHaveClass("highlighted");
        expect(search.getAttribute("aria-activedescendant")).toBe(screen.getByRole("option", { name: /Haiku/ }).id);
        fireEvent.keyDown(search, { key: "ArrowDown" });
        fireEvent.keyDown(search, { key: "ArrowDown" });
        expect(screen.getByRole("option", { name: /Opus/ })).toHaveClass("highlighted");

        fireEvent.keyDown(search, { key: "Enter" });
        expect(onConfig).toHaveBeenCalledWith(expect.objectContaining({ id: "model" }), "opus");
        expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    });

    it("filters models by name, value or description and says when nothing matches", () => {
        const onConfig = vi.fn();
        render(<ComposerPickers agent={codexAgent} onAgent={mocks.onAgent} disabled={false} onConfig={onConfig} model={threeModels} />);
        fireEvent.click(screen.getByRole("button", { name: "Model" }));
        const search = screen.getByRole("combobox", { name: "Search model" });

        fireEvent.change(search, { target: { value: "FAST" } });
        expect(screen.getAllByRole("option").map((option) => option.textContent)).toEqual(["Haiku"]);

        fireEvent.change(search, { target: { value: "gpt" } });
        expect(screen.queryAllByRole("option")).toEqual([]);
        expect(screen.getByText("No matches")).toBeInTheDocument();
        expect(search).not.toHaveAttribute("aria-activedescendant");
        fireEvent.keyDown(search, { key: "ArrowDown" });
        fireEvent.keyDown(search, { key: "Enter" });
        expect(onConfig).not.toHaveBeenCalled();
        expect(screen.getByRole("listbox")).toBeInTheDocument();
    });

    it("does not re-send the model that is already chosen", () => {
        const onConfig = vi.fn();
        render(<ComposerPickers agent={codexAgent} onAgent={mocks.onAgent} disabled={false} onConfig={onConfig} model={threeModels} />);
        fireEvent.click(screen.getByRole("button", { name: "Model" }));
        const current = screen.getByRole("option", { name: /Sonnet/ });

        expect(current).toHaveAttribute("aria-selected", "true");
        fireEvent.click(current);
        expect(onConfig).not.toHaveBeenCalled();
    });

    it("does not pick while an IME is still composing", () => {
        const onConfig = vi.fn();
        render(<ComposerPickers agent={codexAgent} onAgent={mocks.onAgent} disabled={false} onConfig={onConfig} model={threeModels} />);
        fireEvent.click(screen.getByRole("button", { name: "Model" }));
        const search = screen.getByRole("combobox", { name: "Search model" });
        fireEvent.keyDown(search, { key: "ArrowDown" });
        fireEvent.keyDown(search, { key: "Enter", isComposing: true });

        expect(onConfig).not.toHaveBeenCalled();
        expect(screen.getByRole("listbox")).toBeInTheDocument();
    });

    it("closes when the trigger is clicked again, on a click outside, or when focus leaves for elsewhere", () => {
        render(
            <>
                <button type="button">Elsewhere</button>
                <ComposerPickers agent={codexAgent} onAgent={mocks.onAgent} disabled={false} onConfig={() => {}} model={threeModels} />
            </>,
        );
        const trigger = screen.getByRole("button", { name: "Model" });

        fireEvent.click(trigger);
        expect(trigger).toHaveAttribute("aria-expanded", "true");
        fireEvent.click(trigger);
        expect(trigger).toHaveAttribute("aria-expanded", "false");

        fireEvent.click(trigger);
        fireEvent.pointerDown(screen.getByRole("option", { name: /Opus/ }));
        expect(screen.getByRole("listbox")).toBeInTheDocument();
        fireEvent.pointerDown(document.body);
        expect(screen.queryByRole("listbox")).not.toBeInTheDocument();

        fireEvent.click(trigger);
        fireEvent.blur(screen.getByRole("combobox", { name: "Search model" }), { relatedTarget: screen.getByRole("option", { name: /Opus/ }) });
        expect(screen.getByRole("listbox")).toBeInTheDocument();
        fireEvent.blur(screen.getByRole("combobox", { name: "Search model" }), { relatedTarget: screen.getByRole("button", { name: "Elsewhere" }) });
        expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    });

    it("labels empty pickers with the agent's launch model and effort until the session reports its own", () => {
        render(
            <ComposerPickers
                agent={{ ...codexAgent, model: "gpt-6", effort: "xhigh" }}
                onAgent={mocks.onAgent}
                disabled={false}
                onConfig={() => {}}
                effort={noEffort}
            />,
        );

        expect(screen.getByRole("button", { name: "Model" })).toHaveAttribute("title", "gpt-6");
        expect(screen.getByRole("button", { name: "Model" })).toBeEnabled();
        expect(screen.getByRole("button", { name: "Reasoning effort" })).toHaveAttribute("title", "xhigh");
        expect(screen.getByRole("button", { name: "Reasoning effort" })).toBeDisabled();
    });

    it("shows a current value the option list does not name, and the generic labels otherwise", () => {
        const { rerender } = render(
            <ComposerPickers agent={codexAgent} onAgent={mocks.onAgent} disabled={false} onConfig={() => {}} model={picker("mystery", [])} />,
        );
        expect(screen.getByRole("button", { name: "Model" })).toHaveAttribute("title", "mystery");

        rerender(<ComposerPickers agent={codexAgent} onAgent={mocks.onAgent} disabled={false} onConfig={() => {}} effort={noEffort} agentLocked />);
        expect(screen.getByRole("button", { name: "Model" })).toHaveAttribute("title", "Model");
        expect(screen.getByRole("button", { name: "Model" })).toBeDisabled();
        expect(screen.getByRole("button", { name: "Reasoning effort" })).toHaveAttribute("title", "Effort");
    });

    it("ignores a click on the agent that is already running", () => {
        render(<ComposerPickers agent={codexAgent} onAgent={mocks.onAgent} disabled={false} onConfig={() => {}} model={threeModels} />);
        fireEvent.click(screen.getByRole("button", { name: "Model" }));
        fireEvent.click(screen.getByRole("button", { name: /Codex/ }));

        expect(mocks.onAgent).not.toHaveBeenCalled();
        expect(screen.queryByText("Loading models…")).not.toBeInTheDocument();
    });

    it("keeps the picker disabled while the agent switch is still waiting and never picks while loading", () => {
        const onConfig = vi.fn();
        render(<ComposerPickers agent={codexAgent} onAgent={mocks.onAgent} disabled={false} onConfig={onConfig} model={threeModels} />);
        fireEvent.click(screen.getByRole("button", { name: "Model" }));
        fireEvent.click(screen.getByRole("button", { name: /Claude/ }));
        const search = screen.getByRole("combobox", { name: "Search model" });
        fireEvent.keyDown(search, { key: "Enter" });

        expect(screen.getByText("Loading models…")).toBeInTheDocument();
        expect(onConfig).not.toHaveBeenCalled();
    });
});
