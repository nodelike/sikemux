import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { GitModalRenderer } from "./GitModalRenderer";
import { getState, setState } from "../state/store";

afterEach(() => {
    cleanup();
    setState({ gitModal: null });
});

describe("GitModalRenderer confirmations", () => {
    it("makes an explicitly confirmed destructive flow actionable with Enter", async () => {
        const user = userEvent.setup();
        const onConfirm = vi.fn();
        setState({
            gitModal: {
                ownerPaneId: "git-pane",
                kind: "confirm",
                title: "Discard changes",
                body: "This cannot be undone.",
                destructive: true,
                confirmLabel: "discard",
                initialFocus: "confirm",
                confirmKey: "d",
                onConfirm,
            },
        });

        render(<GitModalRenderer paneId="git-pane" active />);
        const discard = screen.getByRole("button", { name: "discard" });
        await waitFor(() => expect(discard).toHaveFocus());

        await user.keyboard("{Enter}");

        expect(onConfirm).toHaveBeenCalledOnce();
        expect(getState().gitModal).toBeNull();
    });

    it("keeps destructive confirmations cancel-focused by default but accepts their explicit key", async () => {
        const user = userEvent.setup();
        const onConfirm = vi.fn();
        setState({
            gitModal: {
                ownerPaneId: "git-pane",
                kind: "confirm",
                title: "Discard changes",
                body: "This cannot be undone.",
                destructive: true,
                confirmLabel: "discard",
                confirmKey: "d",
                onConfirm,
            },
        });

        render(<GitModalRenderer paneId="git-pane" active />);
        await waitFor(() => expect(screen.getByRole("button", { name: "cancel" })).toHaveFocus());

        fireEvent.keyDown(window, { key: "d", repeat: true });
        expect(onConfirm).not.toHaveBeenCalled();
        expect(getState().gitModal).not.toBeNull();

        await user.keyboard("d");
        await user.keyboard("d");

        expect(onConfirm).toHaveBeenCalledOnce();
        expect(getState().gitModal).toBeNull();
    });
});

const menu = (items: { key?: string; label: string; hint?: string; disabled?: boolean; destructive?: boolean; run: () => void }[]) =>
    setState({ gitModal: { ownerPaneId: "git-pane", kind: "menu", title: "Stash", items } });

describe("GitModalRenderer menus", () => {
    it("runs an item picked by click or by its key, and ignores a disabled one", async () => {
        const user = userEvent.setup();
        const apply = vi.fn();
        const drop = vi.fn();
        menu([
            { key: "a", label: "apply stash", hint: "keep stash", run: apply },
            { key: "d", label: "drop stash", destructive: true, disabled: true, run: drop },
        ]);
        render(<GitModalRenderer paneId="git-pane" active />);
        expect(screen.getByRole("dialog", { name: "Stash" })).toBeInTheDocument();
        expect(screen.getByRole("button", { name: /drop stash/ })).toBeDisabled();

        await user.keyboard("d");
        expect(drop).not.toHaveBeenCalled();
        expect(getState().gitModal).not.toBeNull();

        await user.keyboard("a");
        expect(apply).toHaveBeenCalledOnce();
        expect(getState().gitModal).toBeNull();

        act(() => menu([{ label: "pop stash", run: apply }]));
        await user.click(await screen.findByRole("button", { name: "pop stash" }));
        expect(apply).toHaveBeenCalledTimes(2);
        expect(getState().gitModal).toBeNull();
    });

    it("closes on Escape or a click outside, but not a click inside", async () => {
        const user = userEvent.setup();
        menu([{ label: "apply stash", run: vi.fn() }]);
        render(<GitModalRenderer paneId="git-pane" active />);
        await user.click(screen.getByRole("heading", { name: "Stash" }));
        expect(getState().gitModal).not.toBeNull();
        await user.keyboard("{Escape}");
        expect(getState().gitModal).toBeNull();

        act(() => menu([{ label: "apply stash", run: vi.fn() }]));
        await user.click(document.querySelector(".dlg-scrim")!);
        expect(getState().gitModal).toBeNull();
    });

    it("leaves another pane's modal alone and drops its own when it stops being active or unmounts", () => {
        menu([{ label: "apply stash", run: vi.fn() }]);
        const { rerender, unmount } = render(<GitModalRenderer paneId="other-pane" active />);
        expect(screen.queryByRole("dialog")).toBeNull();
        expect(getState().gitModal).not.toBeNull();

        rerender(<GitModalRenderer paneId="git-pane" active={false} />);
        expect(getState().gitModal).toBeNull();

        rerender(<GitModalRenderer paneId="git-pane" active />);
        act(() => menu([{ label: "apply stash", run: vi.fn() }]));
        expect(screen.getByRole("dialog")).toBeInTheDocument();
        unmount();
        expect(getState().gitModal).toBeNull();
    });
});

describe("GitModalRenderer prompts", () => {
    it("submits the typed value with Enter, picking from matching suggestions", async () => {
        const user = userEvent.setup();
        const onConfirm = vi.fn();
        setState({
            gitModal: {
                ownerPaneId: "git-pane",
                kind: "prompt",
                title: "Checkout branch",
                placeholder: "branch name",
                suggestions: [
                    { value: "main", hint: "current" },
                    { value: "feature", hint: "" },
                ],
                onConfirm,
            },
        });
        render(<GitModalRenderer paneId="git-pane" active />);
        const input = screen.getByPlaceholderText("branch name");
        await waitFor(() => expect(input).toHaveFocus());
        await user.type(input, "fea");
        expect(screen.queryByRole("button", { name: /main/ })).toBeNull();
        await user.click(screen.getByRole("button", { name: "feature" }));
        expect(input).toHaveValue("feature");
        expect(input).toHaveFocus();
        await user.keyboard("{Enter}");
        expect(onConfirm).toHaveBeenCalledWith("feature");
        expect(getState().gitModal).toBeNull();
    });

    it("starts from the initial value and cancels without answering", async () => {
        const user = userEvent.setup();
        const onConfirm = vi.fn();
        setState({ gitModal: { ownerPaneId: "git-pane", kind: "prompt", title: "Rename", initial: "old", onConfirm } });
        render(<GitModalRenderer paneId="git-pane" active />);
        expect(screen.getByRole("textbox")).toHaveValue("old");
        await user.click(screen.getByRole("button", { name: "cancel" }));
        expect(onConfirm).not.toHaveBeenCalled();
        expect(getState().gitModal).toBeNull();
    });

    it("takes a multi-line answer only on the primary shortcut", async () => {
        const user = userEvent.setup();
        const onConfirm = vi.fn();
        setState({ gitModal: { ownerPaneId: "git-pane", kind: "prompt", title: "Message", multiline: true, onConfirm } });
        render(<GitModalRenderer paneId="git-pane" active />);
        const input = screen.getByRole("textbox");
        await user.type(input, "one{Enter}two");
        expect(onConfirm).not.toHaveBeenCalled();
        fireEvent.keyDown(input, { key: "Enter", ctrlKey: true });
        expect(onConfirm).toHaveBeenCalledWith("one\ntwo");
    });

    it("submits from the ok button", async () => {
        const user = userEvent.setup();
        const onConfirm = vi.fn();
        setState({ gitModal: { ownerPaneId: "git-pane", kind: "prompt", title: "Name", onConfirm } });
        render(<GitModalRenderer paneId="git-pane" active />);
        await user.type(screen.getByRole("textbox"), "x");
        await user.click(screen.getByRole("button", { name: "ok (↵)" }));
        expect(onConfirm).toHaveBeenCalledWith("x");
    });
});

describe("GitModalRenderer confirmations and cheatsheets", () => {
    it("focuses confirm for a safe action and runs it from the button", async () => {
        const user = userEvent.setup();
        const onConfirm = vi.fn();
        setState({ gitModal: { ownerPaneId: "git-pane", kind: "confirm", title: "Revert?", body: "Creates a commit.", onConfirm } });
        render(<GitModalRenderer paneId="git-pane" active />);
        const confirm = screen.getByRole("button", { name: "confirm" });
        await waitFor(() => expect(confirm).toHaveFocus());
        expect(confirm).not.toHaveClass("danger");
        await user.click(confirm);
        expect(onConfirm).toHaveBeenCalledOnce();
    });

    it("ignores its confirm key while a modifier is held", () => {
        const first = vi.fn();
        setState({
            gitModal: { ownerPaneId: "git-pane", kind: "confirm", title: "Drop?", body: "", confirmKey: "d", onConfirm: first },
        });
        render(<GitModalRenderer paneId="git-pane" active />);
        fireEvent.keyDown(window, { key: "d", ctrlKey: true });
        expect(first).not.toHaveBeenCalled();
        expect(getState().gitModal).not.toBeNull();
    });

    it("lists the shortcut sections", () => {
        setState({
            gitModal: {
                ownerPaneId: "git-pane",
                kind: "cheatsheet",
                title: "Git pane keybindings",
                sections: [{ title: "Global", rows: [{ keys: "?", label: "open this cheatsheet" }] }],
            },
        });
        render(<GitModalRenderer paneId="git-pane" active />);
        expect(screen.getByText("Global")).toBeInTheDocument();
        expect(screen.getByText("open this cheatsheet")).toBeInTheDocument();
    });
});
