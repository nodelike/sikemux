import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ComposerPickers } from "./ComposerPickers";
import { effortConfig, sessionConfigs } from "./sessionConfig";
import { getState, setState } from "../state/store";

const mocks = vi.hoisted(() => ({ onAgent: vi.fn() }));
const codexAgent = { id: "a", type: "codex" as const, title: "Codex", startup: "codex" };

function modelSetup(currentValue: string, options: { value: string; name: string; description?: string }[]) {
    return { configOptions: [{ id: "model", type: "select", currentValue, options }] };
}

const threeModels = modelSetup("sonnet", [
    { value: "sonnet", name: "Sonnet" },
    { value: "opus", name: "Opus" },
    { value: "haiku", name: "Haiku", description: "Fastest" },
]);
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

    it("names a model with the release number its description carries", () => {
        expect(
            sessionConfigs({
                configOptions: [
                    {
                        id: "model",
                        type: "select",
                        currentValue: "opus[1m]",
                        options: [
                            { value: "default", name: "Default (recommended)", description: "Opus (1M context)" },
                            {
                                value: "opus[1m]",
                                name: "Opus (1M context)",
                                description: "Opus 5 with 1M context · Best for everyday, complex tasks",
                            },
                            { value: "sonnet", name: "Sonnet", description: "Sonnet 5 · Efficient for routine tasks" },
                            { value: "haiku", name: "Haiku", description: "Haiku 4.5 · Fastest for quick answers" },
                        ],
                    },
                ],
            })[0].options.map((option) => option.label),
        ).toEqual(["Default (recommended)", "Opus 5 (1M context)", "Sonnet 5", "Haiku 4.5"]);
    });

    it("selects the harness for the existing empty chat", () => {
        setState({ providerProfiles: [{ id: "work", name: "Work Claude", provider: "claude", accent: "#fff" }] });
        render(
            <ComposerPickers
                agent={{ id: "a", type: "codex", title: "Codex", startup: "codex" }}
                onAgent={mocks.onAgent}
                setup={{}}
                disabled={false}
                onConfig={() => {}}
            />,
        );
        fireEvent.click(screen.getByRole("button", { name: "Model" }));
        fireEvent.click(screen.getByRole("button", { name: /Work Claude/ }));
        expect(mocks.onAgent).toHaveBeenCalledWith("claude", "work");
    });

    it("keeps the menu open through an agent switch and fills in the new agent's models", () => {
        const codex = { id: "a", type: "codex" as const, title: "Codex", startup: "codex" };
        const claude = { ...codex, type: "claude" as const, title: "Claude" };
        const models = (value: string, name: string) => ({
            configOptions: [{ id: "model", type: "select", currentValue: value, options: [{ value, name }] }],
        });
        const codexSetup = models("astra", "GPT-6 Astra");
        const props = { onAgent: mocks.onAgent, onConfig: () => {} };
        const { rerender } = render(<ComposerPickers {...props} agent={codex} setup={codexSetup} disabled={false} />);
        fireEvent.click(screen.getByRole("button", { name: "Model" }));
        fireEvent.click(screen.getByRole("button", { name: /Claude/ }));
        expect(mocks.onAgent).toHaveBeenCalledWith("claude", expect.anything());
        rerender(<ComposerPickers {...props} agent={claude} setup={codexSetup} disabled={false} />);
        expect(screen.getByText("Loading models…")).toBeInTheDocument();
        rerender(<ComposerPickers {...props} agent={claude} setup={{}} disabled />);
        expect(screen.getByText("Loading models…")).toBeInTheDocument();
        rerender(<ComposerPickers {...props} agent={claude} setup={models("opus", "Opus")} disabled={false} />);
        expect(screen.queryByText("Loading models…")).not.toBeInTheDocument();
        expect(screen.getByRole("option", { name: /Opus/ })).toBeInTheDocument();
        expect(screen.getByRole("button", { name: /Claude/ })).toHaveAttribute("aria-pressed", "true");
    });

    it("finds effort by its category whatever the agent calls it", () => {
        const configs = sessionConfigs({
            configOptions: [
                { id: "model", category: "model", type: "select", currentValue: "m", options: [] },
                { id: "thinking", category: "thought_level", type: "select", currentValue: "auto", options: [{ value: "auto", name: "Auto" }] },
            ],
        });
        expect(effortConfig(configs, "omp")?.id).toBe("thinking");
        expect(effortConfig(configs, "claude")?.id).toBe("thinking");
        expect(
            effortConfig(sessionConfigs({ configOptions: [{ id: "effort", type: "select", currentValue: "high", options: [] }] }), "claude")?.id,
        ).toBe("effort");
    });

    it("offers no effort picker to an agent whose model has none", () => {
        render(
            <ComposerPickers
                agent={{ id: "a", type: "hermes", title: "Hermes", startup: "hermes" }}
                onAgent={mocks.onAgent}
                disabled={false}
                onConfig={() => {}}
                setup={{ configOptions: [{ id: "model", type: "select", currentValue: "m", options: [{ value: "m", name: "M" }] }] }}
            />,
        );
        expect(screen.getByRole("button", { name: "Model" })).toBeInTheDocument();
        expect(screen.queryByRole("button", { name: "Reasoning effort" })).not.toBeInTheDocument();
    });

    it("lists a harness once when its built-in profile is the default", () => {
        render(
            <ComposerPickers
                agent={{ id: "a", type: "codex", title: "Codex", startup: "codex" }}
                onAgent={mocks.onAgent}
                setup={{}}
                disabled={false}
                onConfig={() => {}}
            />,
        );
        fireEvent.click(screen.getByRole("button", { name: "Model" }));
        const agents = screen.getByRole("group", { name: "Agent" }).querySelectorAll("button");
        expect(Array.from(agents, (button) => button.textContent)).toEqual(["Codex", "Claude"]);
        expect(screen.getByRole("button", { name: /Codex/ })).toHaveAttribute("aria-pressed", "true");
    });

    it("drops the agent picker after messages while keeping model and effort available", () => {
        render(
            <ComposerPickers
                agent={{ id: "a", type: "codex", title: "Codex", startup: "codex" }}
                onAgent={mocks.onAgent}
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
                agent={{ id: "a", type: "codex", title: "Codex", startup: "codex" }}
                onAgent={mocks.onAgent}
                disabled={false}
                onConfig={onConfig}
                setup={{
                    configOptions: [
                        {
                            id: "model",
                            type: "select",
                            currentValue: "sonnet",
                            options: [
                                { value: "sonnet", name: "Sonnet", description: "Sonnet 5 · Efficient for routine tasks" },
                                { value: "opus", name: "Opus" },
                            ],
                        },
                    ],
                }}
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
                agent={{ id: "a", type: "codex", title: "Codex", startup: "codex" }}
                onAgent={mocks.onAgent}
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

    it("reads only well-formed select options from what the agent reports", () => {
        expect(sessionConfigs({})).toEqual([]);
        expect(sessionConfigs({ configOptions: "model" })).toEqual([]);
        expect(
            sessionConfigs({
                configOptions: [
                    null,
                    ["nested"],
                    { id: "mode", type: "toggle", currentValue: "on" },
                    { type: "select", currentValue: "x" },
                    { id: "model", type: "select", currentValue: 3 },
                    {
                        id: "effort",
                        type: "select",
                        category: "thought_level",
                        currentValue: "high",
                        options: [null, { value: "high", name: "High" }, { value: 1, name: "Bad" }, { value: "low" }],
                    },
                    { id: "speed", name: "Speed", type: "select", currentValue: "fast", options: "fast" },
                    { id: "model", type: "select", currentValue: "m", options: [{ options: [{ value: "m", name: "M" }] }] },
                ],
            }),
        ).toEqual([
            {
                id: "effort",
                name: "effort",
                category: "thought_level",
                currentValue: "high",
                options: [{ value: "high", label: "High", description: undefined }],
            },
            { id: "speed", name: "Speed", currentValue: "fast", options: [] },
            { id: "model", name: "model", currentValue: "m", options: [{ value: "m", label: "M", description: undefined }] },
        ]);
    });

    it("leaves a model's name alone when its description names another family or the number is already there", () => {
        const labels = sessionConfigs(
            modelSetup("a", [
                { value: "a", name: "Sonnet", description: "Opus 5 behind the scenes" },
                { value: "b", name: "Opus 5", description: "Opus 5 with 1M context" },
                { value: "c", name: "Haiku", description: "fast and cheap" },
            ]),
        )[0].options.map((option) => option.label);

        expect(labels).toEqual(["Sonnet", "Opus 5", "Haiku"]);
    });

    it("finds no effort option for an agent that does not chat unless one is tagged", () => {
        const configs = sessionConfigs({ configOptions: [{ id: "effort", type: "select", currentValue: "high", options: [] }] });

        expect(effortConfig(configs, "pi")).toBeUndefined();
        expect(effortConfig(configs, "codex")).toBeUndefined();
    });

    it("moves through models with the arrow keys, wrapping at each end, and picks one with Enter", () => {
        const onConfig = vi.fn();
        render(<ComposerPickers agent={codexAgent} onAgent={mocks.onAgent} disabled={false} onConfig={onConfig} setup={threeModels} />);
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
        render(<ComposerPickers agent={codexAgent} onAgent={mocks.onAgent} disabled={false} onConfig={onConfig} setup={threeModels} />);
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
        render(<ComposerPickers agent={codexAgent} onAgent={mocks.onAgent} disabled={false} onConfig={onConfig} setup={threeModels} />);
        fireEvent.click(screen.getByRole("button", { name: "Model" }));
        const current = screen.getByRole("option", { name: /Sonnet/ });

        expect(current).toHaveAttribute("aria-selected", "true");
        fireEvent.click(current);
        expect(onConfig).not.toHaveBeenCalled();
    });

    it("does not pick while an IME is still composing", () => {
        const onConfig = vi.fn();
        render(<ComposerPickers agent={codexAgent} onAgent={mocks.onAgent} disabled={false} onConfig={onConfig} setup={threeModels} />);
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
                <ComposerPickers agent={codexAgent} onAgent={mocks.onAgent} disabled={false} onConfig={() => {}} setup={threeModels} />
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

    it("anchors the menu above its trigger on a narrow window", () => {
        const width = window.innerWidth;
        Object.defineProperty(window, "innerWidth", { configurable: true, value: 600 });
        try {
            render(<ComposerPickers agent={codexAgent} onAgent={mocks.onAgent} disabled={false} onConfig={() => {}} setup={threeModels} />);
            const trigger = screen.getByRole("button", { name: "Model" });
            vi.spyOn(trigger, "getBoundingClientRect").mockReturnValue({ top: 500 } as DOMRect);
            fireEvent.click(trigger);

            const menu = screen.getByRole("listbox").parentElement as HTMLElement;
            expect(menu.style.bottom).toBe(`${window.innerHeight - 500 + 12}px`);
        } finally {
            Object.defineProperty(window, "innerWidth", { configurable: true, value: width });
        }
    });

    it("labels empty pickers with the agent's launch model and effort until the session reports its own", () => {
        render(
            <ComposerPickers
                agent={{ ...codexAgent, model: "gpt-6", effort: "xhigh" }}
                onAgent={mocks.onAgent}
                disabled={false}
                onConfig={() => {}}
                setup={{}}
            />,
        );

        expect(screen.getByRole("button", { name: "Model" })).toHaveAttribute("title", "gpt-6");
        expect(screen.getByRole("button", { name: "Model" })).toBeEnabled();
        expect(screen.getByRole("button", { name: "Reasoning effort" })).toHaveAttribute("title", "xhigh");
        expect(screen.getByRole("button", { name: "Reasoning effort" })).toBeDisabled();
    });

    it("shows a current value the option list does not name, and the generic labels otherwise", () => {
        const { rerender } = render(
            <ComposerPickers agent={codexAgent} onAgent={mocks.onAgent} disabled={false} onConfig={() => {}} setup={modelSetup("mystery", [])} />,
        );
        expect(screen.getByRole("button", { name: "Model" })).toHaveAttribute("title", "mystery");

        rerender(<ComposerPickers agent={codexAgent} onAgent={mocks.onAgent} disabled={false} onConfig={() => {}} setup={{}} agentLocked />);
        expect(screen.getByRole("button", { name: "Model" })).toHaveAttribute("title", "Model");
        expect(screen.getByRole("button", { name: "Model" })).toBeDisabled();
        expect(screen.getByRole("button", { name: "Reasoning effort" })).toHaveAttribute("title", "Effort");
    });

    it("lets an effort picker change the effort", () => {
        const onConfig = vi.fn();
        render(
            <ComposerPickers
                agent={{ id: "a", type: "claude", title: "Claude", startup: "claude" }}
                onAgent={mocks.onAgent}
                disabled={false}
                onConfig={onConfig}
                setup={{
                    configOptions: [
                        {
                            id: "effort",
                            type: "select",
                            currentValue: "high",
                            options: [
                                { value: "high", name: "High", description: "Thinks longer" },
                                { value: "low", name: "Low" },
                            ],
                        },
                    ],
                }}
            />,
        );
        fireEvent.click(screen.getByRole("button", { name: "Reasoning effort" }));
        expect(screen.getByText("Thinks longer")).toBeInTheDocument();
        expect(screen.queryByRole("group", { name: "Agent" })).not.toBeInTheDocument();
        fireEvent.click(screen.getByRole("option", { name: /Low/ }));

        expect(onConfig).toHaveBeenCalledWith(expect.objectContaining({ id: "effort" }), "low");
    });

    it("ignores a click on the agent that is already running", () => {
        render(<ComposerPickers agent={codexAgent} onAgent={mocks.onAgent} disabled={false} onConfig={() => {}} setup={threeModels} />);
        fireEvent.click(screen.getByRole("button", { name: "Model" }));
        fireEvent.click(screen.getByRole("button", { name: /Codex/ }));

        expect(mocks.onAgent).not.toHaveBeenCalled();
        expect(screen.queryByText("Loading models…")).not.toBeInTheDocument();
    });

    it("keeps the picker disabled while the agent switch is still waiting and never picks while loading", () => {
        const onConfig = vi.fn();
        render(<ComposerPickers agent={codexAgent} onAgent={mocks.onAgent} disabled={false} onConfig={onConfig} setup={threeModels} />);
        fireEvent.click(screen.getByRole("button", { name: "Model" }));
        fireEvent.click(screen.getByRole("button", { name: /Claude/ }));
        const search = screen.getByRole("combobox", { name: "Search model" });
        fireEvent.keyDown(search, { key: "Enter" });

        expect(screen.getByText("Loading models…")).toBeInTheDocument();
        expect(onConfig).not.toHaveBeenCalled();
    });
});
