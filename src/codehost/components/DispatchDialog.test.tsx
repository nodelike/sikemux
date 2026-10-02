import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({ branches: vi.fn(() => Promise.resolve(["main", "dev"])), dispatch: vi.fn(() => new Promise(() => {})) }));

import { invalidate } from "../../plugin-api/resources";
import { useToasts } from "../../state/toast";
import { DispatchDialog } from "./DispatchDialog";
import { InHost, registerTestHost, TEST_HOST } from "../testHost";

const host = registerTestHost(api);
const wrapper = ({ children }: { children: React.ReactNode }) => <InHost host={host}>{children}</InHost>;

const workflow = { id: "1", name: "Deploy", path: ".github/workflows/deploy.yml", state: "active", active: true, url: "" };

const repo = { provider: TEST_HOST, owner: "a", name: "b" };
const toasts = () => useToasts.getState().toasts.map((toast) => toast.text);

beforeEach(() => {
    invalidate(() => true);
    useToasts.setState({ toasts: [] });
    api.dispatch.mockReset().mockReturnValue(new Promise(() => {}));
});

afterEach(cleanup);

async function renderDialog(defaultBranch: string | null = "main") {
    const onClose = vi.fn();
    render(<DispatchDialog repo={repo} workflow={workflow} defaultBranch={defaultBranch} onClose={onClose} />, { wrapper });
    await act(async () => {});
    return onClose;
}

describe("DispatchDialog", () => {
    it("is a modal dialog that closes on Escape", async () => {
        const onClose = vi.fn();
        render(<DispatchDialog repo={{ provider: TEST_HOST, owner: "a", name: "b" }} workflow={workflow} defaultBranch="main" onClose={onClose} />, {
            wrapper,
        });
        await act(async () => {});
        const dialog = screen.getByRole("dialog", { name: "Run Deploy" });
        expect(dialog.getAttribute("aria-modal")).toBe("true");
        fireEvent.keyDown(screen.getByLabelText("Branch or tag"), { key: "Escape" });
        expect(onClose).toHaveBeenCalledTimes(1);
    });

    it("starts the workflow once however many times it is asked", async () => {
        render(<DispatchDialog repo={{ provider: TEST_HOST, owner: "a", name: "b" }} workflow={workflow} defaultBranch="main" onClose={() => {}} />, {
            wrapper,
        });
        const run = screen.getByRole("button", { name: "Run workflow" });
        await act(async () => {
            fireEvent.click(run);
            fireEvent.click(run);
        });
        expect(api.dispatch).toHaveBeenCalledTimes(1);
    });
});

describe("starting a workflow", () => {
    it("sends the branch and every named input, then closes", async () => {
        api.dispatch.mockResolvedValue(undefined);
        const onClose = await renderDialog(" main ");
        fireEvent.click(screen.getByRole("button", { name: /Add an input/ }));
        fireEvent.change(screen.getByRole("textbox", { name: "Input name" }), { target: { value: " env " } });
        fireEvent.change(screen.getByRole("textbox", { name: "Value of env" }), { target: { value: "prod" } });
        fireEvent.click(screen.getByRole("button", { name: /Add an input/ }));
        fireEvent.change(screen.getAllByRole("textbox", { name: "Value of input" })[0], { target: { value: "ignored" } });
        await act(async () => {
            fireEvent.click(screen.getByRole("button", { name: "Run workflow" }));
        });
        expect(api.dispatch).toHaveBeenCalledWith(repo, "1", "main", { env: "prod" });
        expect(toasts()).toContain("Started Deploy on main");
        expect(onClose).toHaveBeenCalledTimes(1);
    });

    it("drops an input that is removed", async () => {
        api.dispatch.mockResolvedValue(undefined);
        await renderDialog();
        fireEvent.click(screen.getByRole("button", { name: /Add an input/ }));
        fireEvent.change(screen.getByRole("textbox", { name: "Input name" }), { target: { value: "env" } });
        fireEvent.click(screen.getByRole("button", { name: "Remove env" }));
        expect(screen.queryByRole("textbox", { name: "Input name" })).toBeNull();
        await act(async () => {
            fireEvent.submit(screen.getByLabelText("Branch or tag"));
        });
        expect(api.dispatch).toHaveBeenCalledWith(repo, "1", "main", {});
    });

    it("needs a branch before it can start", async () => {
        await renderDialog(null);
        expect(screen.getByRole("button", { name: "Run workflow" })).toHaveProperty("disabled", true);
        await act(async () => {
            fireEvent.submit(screen.getByLabelText("Branch or tag"));
        });
        expect(api.dispatch).not.toHaveBeenCalled();
    });

    it("says it is starting while the host is asked", async () => {
        await renderDialog();
        await act(async () => {
            fireEvent.click(screen.getByRole("button", { name: "Run workflow" }));
        });
        expect(screen.getByRole("button", { name: "Starting…" })).toHaveProperty("disabled", true);
    });

    it("stays open and says why when the host refuses", async () => {
        api.dispatch.mockRejectedValue(new Error("Workflow does not have 'workflow_dispatch' trigger"));
        const onClose = await renderDialog();
        await act(async () => {
            fireEvent.click(screen.getByRole("button", { name: "Run workflow" }));
        });
        expect(toasts()).toContain("Could not start Deploy: Workflow does not have 'workflow_dispatch' trigger");
        expect(onClose).not.toHaveBeenCalled();
        expect(screen.getByRole("button", { name: "Run workflow" })).toHaveProperty("disabled", false);
    });

    it("suggests the repository's branches", async () => {
        await renderDialog();
        const options = [...document.querySelectorAll("datalist option")].map((option) => option.getAttribute("value"));
        expect(options).toEqual(["main", "dev"]);
    });

    it("closes from Cancel or a press outside, but not a press inside or another key", async () => {
        const onClose = await renderDialog();
        fireEvent.keyDown(screen.getByLabelText("Branch or tag"), { key: "a" });
        fireEvent.mouseDown(screen.getByRole("dialog"));
        expect(onClose).not.toHaveBeenCalled();
        fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
        expect(onClose).toHaveBeenCalledTimes(1);
        fireEvent.mouseDown(document.querySelector(".dlg-scrim") as HTMLElement);
        expect(onClose).toHaveBeenCalledTimes(2);
    });
});
