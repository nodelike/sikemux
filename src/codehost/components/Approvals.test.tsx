import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PendingApproval } from "../api";

const api = vi.hoisted(() => ({ pendingApprovals: vi.fn(), reviewDeployment: vi.fn() }));

import { invalidate } from "../../plugin-api/resources";
import { acceptDialog, dismissDialog, useDialogs } from "../../state/dialog";
import { useToasts } from "../../state/toast";
import { InHost, registerTestHost, TEST_HOST } from "../testHost";
import { Approvals, mayBeWaiting } from "./Approvals";

const host = registerTestHost(api);
const repo = { provider: TEST_HOST, owner: "nodelike", name: "sikemux" };

const pending = (overrides: Partial<PendingApproval> = {}): PendingApproval => ({
    environmentId: 1,
    environment: "production",
    waitMinutes: 0,
    canApprove: true,
    reviewers: [],
    ...overrides,
});

const toasts = () => useToasts.getState().toasts.map((toast) => toast.text);

function answerDialog(yes: boolean) {
    const dialog = useDialogs.getState().dialog;
    if (!dialog) throw new Error("no dialog is open");
    act(() => (yes ? acceptDialog(dialog.id) : dismissDialog(dialog.id)));
}

async function renderApprovals(found: PendingApproval[], status = "waiting", conclusion: string | null = null) {
    api.pendingApprovals.mockResolvedValue(found);
    const view = render(
        <InHost host={host}>
            <Approvals repo={repo} runId="7" status={status} conclusion={conclusion} active />
        </InHost>,
    );
    await act(async () => {});
    return view;
}

beforeEach(() => {
    invalidate(() => true);
    useToasts.setState({ toasts: [] });
    api.pendingApprovals.mockReset();
    api.reviewDeployment.mockReset().mockResolvedValue(undefined);
});

afterEach(cleanup);

describe("mayBeWaiting", () => {
    it("only counts a run GitHub is holding", () => {
        expect(mayBeWaiting("waiting", null)).toBe(true);
        expect(mayBeWaiting("action_required", null)).toBe(true);
        expect(mayBeWaiting("completed", "action_required")).toBe(true);
        expect(mayBeWaiting("in_progress", null)).toBe(false);
        expect(mayBeWaiting("completed", "success")).toBe(false);
    });
});

describe("Approvals", () => {
    it("does not ask about a run nobody is holding", async () => {
        const view = await renderApprovals([pending()], "in_progress");
        expect(api.pendingApprovals).not.toHaveBeenCalled();
        expect(view.container.textContent).toBe("");
    });

    it("shows nothing when nothing is waiting", async () => {
        const view = await renderApprovals([]);
        expect(api.pendingApprovals).toHaveBeenCalledWith(repo, "7");
        expect(view.container.textContent).toBe("");
    });

    it("names the environments waiting and the longest wait", async () => {
        await renderApprovals([pending(), pending({ environmentId: 2, environment: "staging", waitMinutes: 15 })]);
        expect(screen.getByText("production, staging · 15m wait")).toBeTruthy();
    });

    it("approves every environment the person may sign off", async () => {
        await renderApprovals([pending(), pending({ environmentId: 2, environment: "staging", canApprove: false })]);
        fireEvent.click(screen.getByRole("button", { name: "Approve" }));
        await act(async () => {});
        expect(api.reviewDeployment).toHaveBeenCalledWith(repo, "7", [1], "approved");
        expect(toasts()).toContain("Approved production");
    });

    it("rejects only once the person confirms", async () => {
        await renderApprovals([pending()]);
        fireEvent.click(screen.getByRole("button", { name: "Reject" }));
        await act(async () => {});
        expect(useDialogs.getState().dialog).toMatchObject({ title: "Reject this deployment?", body: "production will not run." });
        answerDialog(false);
        await act(async () => {});
        expect(api.reviewDeployment).not.toHaveBeenCalled();

        fireEvent.click(screen.getByRole("button", { name: "Reject" }));
        await act(async () => {});
        answerDialog(true);
        await act(async () => {});
        expect(api.reviewDeployment).toHaveBeenCalledWith(repo, "7", [1], "rejected");
        expect(toasts()).toContain("Rejected production");
    });

    it("holds both buttons while the answer is on its way, and says why it failed", async () => {
        let fail: (error: Error) => void = () => {};
        api.reviewDeployment.mockReturnValue(new Promise((_, reject) => (fail = reject)));
        await renderApprovals([pending()]);
        fireEvent.click(screen.getByRole("button", { name: "Approve" }));
        await act(async () => {});
        expect(screen.getByRole("button", { name: "Approve" })).toHaveProperty("disabled", true);
        expect(screen.getByRole("button", { name: "Reject" })).toHaveProperty("disabled", true);
        await act(async () => fail(new Error("not allowed")));
        expect(toasts()).toContain("Could not answer the deployment: not allowed");
        expect(screen.getByRole("button", { name: "Approve" })).toHaveProperty("disabled", false);
    });

    it("names who has to sign off when the person cannot", async () => {
        await renderApprovals([pending({ canApprove: false, reviewers: ["alice", "ops-team"] })]);
        expect(screen.getByText("alice, ops-team")).toBeTruthy();
        expect(screen.queryByRole("button", { name: "Approve" })).toBeNull();
    });

    it("says someone else has to sign off when nobody is named", async () => {
        await renderApprovals([pending({ canApprove: false })]);
        expect(screen.getByText("Someone else has to sign this off.")).toBeTruthy();
    });
});
