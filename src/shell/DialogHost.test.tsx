import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { DialogHost } from "./DialogHost";
import { confirmDialog, promptDialog, resetDialogsForTests } from "../state/dialog";

afterEach(() => {
    resetDialogsForTests();
    cleanup();
});

describe("DialogHost", () => {
    it("resolves a confirm from the button and from Escape", async () => {
        const user = userEvent.setup();
        render(<DialogHost />);

        const accepted = confirmDialog({ title: "Remove worktree?", body: "Dirty worktrees will be refused." });
        await screen.findByRole("dialog", { name: "Remove worktree?" });
        expect(screen.getByText("Dirty worktrees will be refused.")).toBeInTheDocument();

        await user.click(screen.getByRole("button", { name: "Confirm" }));
        await expect(accepted).resolves.toBe(true);
        expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

        const dismissed = confirmDialog({ title: "Discard unsaved changes?" });
        await screen.findByRole("dialog");
        await user.keyboard("{Escape}");
        await expect(dismissed).resolves.toBe(false);
    });

    it("shows the commands a confirm would allow", async () => {
        render(<DialogHost />);
        void confirmDialog({ title: "Trust this sikemux.json?", commands: [{ label: "Task · Web", command: "pnpm dev" }] });

        expect(await screen.findByText("Task · Web")).toBeInTheDocument();
        expect(screen.getByText("pnpm dev", { selector: "code" })).toBeInTheDocument();
    });

    it("focuses cancel first on a destructive confirm", async () => {
        render(<DialogHost />);
        void confirmDialog({ title: "Move to Trash?", destructive: true, confirmLabel: "Delete" });

        await waitFor(() => expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus());
    });

    it("returns the typed value from a prompt and null when dismissed", async () => {
        const user = userEvent.setup();
        render(<DialogHost />);

        const named = promptDialog({ title: "New request", label: "Name", initial: "draft" });
        const input = await screen.findByLabelText("Name");
        await user.clear(input);
        await user.type(input, "get-users{Enter}");
        await expect(named).resolves.toBe("get-users");

        const cancelled = promptDialog({ title: "Rename request" });
        await screen.findByRole("dialog");
        await user.click(screen.getByRole("button", { name: "Cancel" }));
        await expect(cancelled).resolves.toBeNull();
    });

    it("shows queued dialogs one at a time in request order", async () => {
        const user = userEvent.setup();
        render(<DialogHost />);

        const first = confirmDialog({ title: "First" });
        const second = confirmDialog({ title: "Second" });

        await screen.findByRole("dialog", { name: "First" });
        expect(screen.queryByRole("dialog", { name: "Second" })).not.toBeInTheDocument();

        await user.click(screen.getByRole("button", { name: "Confirm" }));
        await expect(first).resolves.toBe(true);

        await screen.findByRole("dialog", { name: "Second" });
        await user.click(screen.getByRole("button", { name: "Cancel" }));
        await expect(second).resolves.toBe(false);
    });
});
