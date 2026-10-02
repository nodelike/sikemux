import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { HostAccount } from "../registry";

const api = vi.hoisted(() => ({
    status: vi.fn(),
    workflows: vi.fn(),
    rateLimit: vi.fn(() => new Promise(() => {})),
}));

const stub = vi.hoisted(() => (name: string) => (props: Record<string, unknown>) => (
    <div data-testid={name}>
        {Object.entries(props)
            .filter(([, value]) => typeof value !== "function" && typeof value !== "object")
            .map(([key, value]) => `${key}=${String(value)}`)
            .join(" ")}
    </div>
));

vi.mock("./PullsView", () => ({ PullsView: stub("pulls") }));
vi.mock("./IssuesView", () => ({ IssuesView: stub("issues") }));
vi.mock("./ReleasesView", () => ({ ReleasesView: stub("releases") }));
vi.mock("./InboxView", () => ({ InboxView: stub("inbox") }));
vi.mock("./RunView", () => ({ RunView: stub("run") }));
vi.mock("./RunsList", () => ({
    RunsList: (props: { branch: string | null; canWrite: boolean; onDispatch: (id: string) => void }) => (
        <div data-testid="runs">
            branch={String(props.branch)} canWrite={String(props.canWrite)}
            <button type="button" onClick={() => props.onDispatch("12")}>
                Run workflow
            </button>
        </div>
    ),
}));
vi.mock("./DispatchDialog", () => ({
    DispatchDialog: (props: { workflow: { name: string }; defaultBranch: string | null; onClose: () => void }) => (
        <div role="dialog" aria-label="dispatch">
            {props.workflow.name} on {String(props.defaultBranch)}
            <button type="button" onClick={props.onClose}>
                Close
            </button>
        </div>
    ),
}));

import { invalidate } from "../../plugin-api/resources";
import type { Section } from "../state";
import { hostSettings, setFollowBranch, updateView, useHostViews, viewOf } from "../state";
import { InHost, registerTestHost, TEST_HOST } from "../testHost";
import { HostArea } from "./HostArea";

const signIn = vi.hoisted(() => ({ last: null as null | ((account: string | null) => void) }));
const host = {
    ...registerTestHost(api),
    SignIn: ({ onSignedIn }: { onSignedIn: (account: string | null) => void }) => {
        signIn.last = onSignedIn;
        return <div>sign in here</div>;
    },
};
const repo = { provider: TEST_HOST, owner: "nodelike", name: "sikemux" };
const PANE = "p-host";
const CWD = "/work/sikemux";

const signedIn: HostAccount = { id: "ada-id", ok: true, login: "ada", avatarUrl: null, host: "example.test", canWriteCi: true, warning: null };

function show(section: Section, extra: { branch?: string | null; cwd?: string | null; repo?: typeof repo & { account?: string } } = {}) {
    const props = {
        paneId: PANE,
        section,
        repo: extra.repo ?? repo,
        branch: extra.branch === undefined ? "feat/x" : extra.branch,
        cwd: extra.cwd === undefined ? CWD : extra.cwd,
        active: true,
    };
    const view = render(
        <InHost host={host}>
            <HostArea {...props} />
        </InHost>,
    );
    return {
        ...view,
        showAgain: (next: Partial<typeof props>) =>
            view.rerender(
                <InHost host={host}>
                    <HostArea {...props} {...next} />
                </InHost>,
            ),
    };
}

beforeEach(() => {
    invalidate(() => true);
    useHostViews.setState({ views: {} });
    api.status.mockReset().mockResolvedValue(signedIn);
    api.workflows.mockReset().mockResolvedValue([{ id: "12", name: "Deploy", path: ".github/workflows/deploy.yml", state: "active" }]);
    hostSettings(TEST_HOST).update((settings) => ({ ...settings, accountByProject: {}, followBranch: true }));
    signIn.last = null;
});

afterEach(cleanup);

describe("HostArea", () => {
    it("connects before it shows anything", () => {
        api.status.mockReturnValue(new Promise(() => {}));
        show("pulls", { repo: { ...repo, account: "slow-id" } });
        expect(screen.getByRole("status", { name: "Connecting to Test host" })).toBeTruthy();
    });

    it("says why the host could not be reached and tries again when asked", async () => {
        api.status.mockRejectedValueOnce({ category: "network", message: "offline" }).mockResolvedValue(signedIn);
        show("pulls");
        expect(await screen.findByText("Could not reach Test host")).toBeTruthy();
        expect(screen.getByText("offline")).toBeTruthy();
        await userEvent.click(screen.getByRole("button", { name: "Try again" }));
        expect(await screen.findByTestId("pulls")).toBeTruthy();
    });

    it("asks to sign in, and keeps the account it signs in as for this project", async () => {
        api.status.mockResolvedValue({ ...signedIn, ok: false });
        show("pulls");
        await screen.findByText("sign in here");
        act(() => signIn.last?.("grace-id"));
        expect(hostSettings(TEST_HOST).get().accountByProject[CWD]).toBe("grace-id");
    });

    it("keeps no account for a repository that is not the project's own", async () => {
        api.status.mockResolvedValue({ ...signedIn, ok: false });
        show("pulls", { cwd: null });
        await screen.findByText("sign in here");
        act(() => signIn.last?.("grace-id"));
        act(() => signIn.last?.(null));
        expect(hostSettings(TEST_HOST).get().accountByProject).toEqual({});
    });

    it("shows the section asked for, as the signed-in account", async () => {
        updateView(PANE, { item: 7, composing: "pull", pullState: "closed" });
        show("pulls");
        expect((await screen.findByTestId("pulls")).textContent).toBe(
            `paneId=${PANE} listState=closed item=7 composing=true projectBranch=feat/x cwd=${CWD} login=ada active=true`,
        );
        cleanup();
        show("issues");
        expect((await screen.findByTestId("issues")).textContent).toContain("composing=false");
        cleanup();
        show("releases");
        expect(await screen.findByTestId("releases")).toBeTruthy();
        cleanup();
        show("inbox");
        expect((await screen.findByTestId("inbox")).textContent).toContain("login=ada");
    });

    it("lists runs on the project's branch while following it, and on a branch typed into the filter otherwise", async () => {
        show("actions");
        expect((await screen.findByTestId("runs")).textContent).toContain("branch=feat/x canWrite=true");
        act(() => updateView(PANE, { branch: "main" }));
        expect(screen.getByTestId("runs").textContent).toContain("branch=main");
        act(() => {
            updateView(PANE, { branch: null });
            setFollowBranch(TEST_HOST, false);
        });
        expect(screen.getByTestId("runs").textContent).toContain("branch=null");
    });

    it("cannot start runs as an account that may not", async () => {
        api.status.mockResolvedValue({ ...signedIn, canWriteCi: false });
        show("actions");
        expect((await screen.findByTestId("runs")).textContent).toContain("canWrite=false");
    });

    it("opens a run in place of the list", async () => {
        updateView(PANE, { run: "99", job: "5" });
        show("actions");
        expect((await screen.findByTestId("run")).textContent).toContain("runId=99 openJob=5");
    });

    it("starts a workflow from the list in a dialog on the branch in view", async () => {
        show("actions");
        await userEvent.click(await screen.findByRole("button", { name: "Run workflow" }));
        expect(viewOf(PANE).dispatching).toBe("12");
        expect((await screen.findByRole("dialog", { name: "dispatch" })).textContent).toContain("Deploy on feat/x");
        await userEvent.click(screen.getByRole("button", { name: "Close" }));
        expect(viewOf(PANE).dispatching).toBeNull();
        expect(screen.queryByRole("dialog")).toBeNull();
    });

    it("shows no dialog for a workflow the host no longer has", async () => {
        updateView(PANE, { dispatching: "404" });
        show("actions");
        await waitFor(() => expect(api.workflows).toHaveBeenCalled());
        await screen.findByTestId("runs");
        expect(screen.queryByRole("dialog")).toBeNull();
    });

    it("closes what was open when the pane moves to another repository, but not on a redraw", async () => {
        updateView(PANE, { item: 7 });
        const { showAgain } = show("pulls");
        await screen.findByTestId("pulls");
        showAgain({ branch: "main" });
        expect(viewOf(PANE).item).toBe(7);
        showAgain({ repo: { ...repo, name: "other" } });
        await waitFor(() => expect(viewOf(PANE).item).toBeNull());
    });
});
