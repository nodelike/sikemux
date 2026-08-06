import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { CustomCommand } from "../commands/registry";
import { CommandPalette } from "./CommandPalette";

const custom: CustomCommand = {
    id: "lint",
    title: "Lint project",
    detail: "Run ESLint in a background command",
    command: "pnpm lint",
    contexts: ["project"],
    placement: "background",
};

afterEach(cleanup);

describe("CommandPalette", () => {
    it("searches built-ins and runs the selected command", async () => {
        const user = userEvent.setup();
        const executeBuiltin = vi.fn();
        const onClose = vi.fn();
        const onExecute = vi.fn();
        render(<CommandPalette keybindingOverrides={{}} executeBuiltin={executeBuiltin} context="project" onClose={onClose} onExecute={onExecute} />);

        const input = screen.getByRole("textbox", { name: "Search commands" });
        await user.type(input, "open settings");
        fireEvent.keyDown(input, { key: "Enter" });

        expect(onClose).toHaveBeenCalledOnce();
        expect(onExecute).toHaveBeenCalledWith("builtin:settings.toggle");
        expect(executeBuiltin).toHaveBeenCalledWith("settings.toggle");
    });

    it("runs available custom commands and hides commands from other contexts", async () => {
        const user = userEvent.setup();
        const executeCustom = vi.fn();
        const { rerender } = render(
            <CommandPalette
                keybindingOverrides={{}}
                executeBuiltin={vi.fn()}
                customCommands={[custom]}
                executeCustom={executeCustom}
                context="project"
                onClose={vi.fn()}
            />,
        );

        await user.click(screen.getByRole("option", { name: /Lint project/ }));
        expect(executeCustom).toHaveBeenCalledWith(custom);

        rerender(
            <CommandPalette
                keybindingOverrides={{}}
                executeBuiltin={vi.fn()}
                customCommands={[custom]}
                executeCustom={executeCustom}
                context="ssh"
                onClose={vi.fn()}
            />,
        );
        expect(screen.queryByRole("option", { name: /Lint project/ })).not.toBeInTheDocument();
    });

    it("wraps keyboard selection and closes with Escape", () => {
        const executeBuiltin = vi.fn();
        const onClose = vi.fn();
        render(<CommandPalette keybindingOverrides={{}} executeBuiltin={executeBuiltin} onClose={onClose} />);
        const input = screen.getByRole("textbox", { name: "Search commands" });

        fireEvent.keyDown(input, { key: "ArrowUp" });
        fireEvent.keyDown(input, { key: "Enter" });
        expect(executeBuiltin).toHaveBeenCalledWith("bruno.environment");

        fireEvent.keyDown(input, { key: "Escape" });
        expect(onClose).toHaveBeenCalledTimes(2);
    });
});
