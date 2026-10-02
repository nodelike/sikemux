import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createRef } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { useGitWorkbench } from "../state/gitWorkbench";
import { GitComposer } from "./GitComposer";

beforeEach(() => useGitWorkbench.setState({ drafts: {}, operations: {} }));
afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
});

function renderComposer(props: Partial<Parameters<typeof GitComposer>[0]> = {}) {
    const handlers = { onCommit: vi.fn(), onGenerate: vi.fn(), onPickAgent: vi.fn() };
    const messageRef = createRef<HTMLTextAreaElement>();
    render(
        <GitComposer
            repo="/repo"
            busy={false}
            generating={false}
            stagedCount={2}
            agentLabel="Claude · opus"
            messageRef={messageRef}
            {...handlers}
            {...props}
        />,
    );
    return { ...handlers, message: screen.getByRole("textbox", { name: "Commit message" }) };
}

it("commits only once there is a message and something staged", async () => {
    const user = userEvent.setup();
    const { onCommit, message } = renderComposer();
    const commit = screen.getByRole("button", { name: "Commit 2 files" });
    expect(commit).toBeDisabled();
    fireEvent.keyDown(message, { key: "Enter", metaKey: true });
    expect(onCommit).not.toHaveBeenCalled();

    await user.type(message, "fix: things");
    expect(useGitWorkbench.getState().drafts["/repo"]).toBe("fix: things");
    expect(commit).toBeEnabled();
    fireEvent.keyDown(message, { key: "Enter", ctrlKey: true });
    expect(onCommit).toHaveBeenCalledOnce();
    await user.click(commit);
    expect(onCommit).toHaveBeenCalledTimes(2);
});

it("names a single staged file, and says nothing is staged", () => {
    useGitWorkbench.setState({ drafts: { "/repo": "msg" } });
    const { rerender } = render(
        <GitComposer
            repo="/repo"
            busy={false}
            generating={false}
            stagedCount={1}
            agentLabel="Claude · opus"
            messageRef={createRef()}
            onCommit={() => {}}
            onGenerate={() => {}}
            onPickAgent={() => {}}
        />,
    );
    expect(screen.getByRole("button", { name: "Commit 1 file" })).toBeEnabled();
    rerender(
        <GitComposer
            repo="/repo"
            busy={false}
            generating={false}
            stagedCount={0}
            agentLabel="Claude · opus"
            messageRef={createRef()}
            onCommit={() => {}}
            onGenerate={() => {}}
            onPickAgent={() => {}}
        />,
    );
    expect(screen.getByRole("button", { name: "Commit" })).toBeDisabled();
});

it("holds the message still while an operation runs", () => {
    useGitWorkbench.setState({ drafts: { "/repo": "fix: things" } });
    const { onCommit, message } = renderComposer({ busy: true, generating: true });
    expect(message).toHaveAttribute("readonly");
    expect(fireEvent.keyDown(message, { key: "Enter", metaKey: true })).toBe(false);
    expect(onCommit).not.toHaveBeenCalled();
    expect(screen.getByText("writing…")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "writing…" })).toBeDisabled();
});

it("gives up focus on Escape and keeps its keys from the pane", () => {
    const outer = vi.fn();
    window.addEventListener("keydown", outer);
    try {
        const { message } = renderComposer();
        message.focus();
        fireEvent.keyDown(message, { key: "Escape" });
        expect(message).not.toHaveFocus();
        expect(outer).not.toHaveBeenCalled();
    } finally {
        window.removeEventListener("keydown", outer);
    }
});

it("generates the message and opens the agent picker from its buttons", async () => {
    const user = userEvent.setup();
    const { onGenerate, onPickAgent } = renderComposer();
    await user.click(screen.getByRole("button", { name: "Claude · opus" }));
    expect(onGenerate).toHaveBeenCalledOnce();
    const pick = screen.getByRole("button", { name: "Pick the agent and model" });
    await user.click(pick);
    expect(onPickAgent).toHaveBeenCalledWith(pick);
});

it("grows the box to fit the message, again when a width change rewraps it", () => {
    let resized: () => void = () => {};
    vi.stubGlobal(
        "ResizeObserver",
        class {
            constructor(callback: () => void) {
                resized = callback;
            }
            observe() {}
            disconnect() {}
        },
    );
    let scrollHeight = 40;
    let clientWidth = 300;
    vi.spyOn(HTMLTextAreaElement.prototype, "scrollHeight", "get").mockImplementation(() => scrollHeight);
    vi.spyOn(HTMLTextAreaElement.prototype, "clientWidth", "get").mockImplementation(() => clientWidth);
    try {
        const { message } = renderComposer();
        expect(message.style.height).toBe("40px");

        scrollHeight = 80;
        act(() => resized());
        expect(message.style.height).toBe("40px");

        clientWidth = 200;
        act(() => resized());
        expect(message.style.height).toBe("80px");
    } finally {
        vi.restoreAllMocks();
    }
});
