import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { HostAccount } from "../registry";

const api = vi.hoisted(() => ({
    status: vi.fn(),
    resolveRemote: vi.fn(),
    accountFor: vi.fn(),
    accounts: vi.fn(() => Promise.resolve([])),
    commitAuthors: vi.fn(),
    image: vi.fn((url: string) => Promise.resolve(`data:${url}`)),
    pulls: vi.fn(() => Promise.resolve([])),
}));

const localGit = vi.hoisted(() => ({ remotes: vi.fn(), overview: vi.fn() }));
vi.mock("../../api/git", async (importOriginal) => ({ ...(await importOriginal<typeof import("../../api/git")>()), git: localGit }));

vi.mock("./HostArea", () => ({
    HostArea: (props: {
        section: string;
        repo: { owner: string; name: string; account?: string | null };
        branch: string | null;
        cwd: string | null;
    }) => (
        <div data-testid="host-area">
            {props.section} {props.repo.owner}/{props.repo.name} as {String(props.repo.account)} on {String(props.branch)} in {String(props.cwd)}
        </div>
    ),
}));
vi.mock("./RepoPicker", () => ({
    RepoPicker: (props: { onPick: (repo: { provider: string; owner: string; name: string }) => void; onClose: () => void }) => (
        <div role="dialog" aria-label="picker">
            <button type="button" onClick={() => props.onPick({ provider: "test.host", owner: "team", name: "fork" })}>
                team/fork
            </button>
            <button type="button" onClick={() => props.onPick({ provider: "test.host", owner: "nodelike", name: "sikemux" })}>
                nodelike/sikemux
            </button>
            <button type="button" onClick={props.onClose}>
                Close picker
            </button>
        </div>
    ),
}));

import { AuthorAvatar } from "../../git/AuthorAvatar";
import { invalidate, useResource } from "../../plugin-api/resources";
import { emit } from "../../state/bus";
import type { GitArea } from "../../state/types";
import { pullsR } from "../resources";
import { hostSettings, setProjectRepo } from "../state";
import { registerTestHost, TEST_HOST } from "../testHost";
import { GitHostShell } from "./GitHostShell";

const signIn = vi.hoisted(() => ({ last: null as null | ((account: string | null) => void) }));
const host = registerTestHost(api) as { -readonly [K in keyof ReturnType<typeof registerTestHost>]: ReturnType<typeof registerTestHost>[K] };
host.SignIn = ({ onSignedIn }) => {
    signIn.last = onSignedIn;
    return <div>sign in here</div>;
};
host.avatarForEmail = (email) => (email.endsWith("@noreply.example") ? "https://example.test/noreply.png" : null);

const CWD = "/work/sikemux";
const remoteUrl = "git@example.test:nodelike/sikemux.git";
const signedIn: HostAccount = { id: "ada-id", ok: true, login: "ada", avatarUrl: null, host: "example.test", canWriteCi: true, warning: null };

function PullCount() {
    const pulls = useResource(pullsR, { provider: TEST_HOST, owner: "nodelike", name: "sikemux" }, "open");
    return <span>pulls read: {pulls.data ? "yes" : "no"}</span>;
}

function shell(area: GitArea, children = <div>local workbench</div>) {
    const onArea = vi.fn();
    const local = [{ id: "changes", label: "Changes", icon: null, on: area === "local", onSelect: vi.fn() }];
    const view = render(
        <GitHostShell paneId="p-git" cwd={CWD} area={area} active onArea={onArea} local={local}>
            {children}
        </GitHostShell>,
    );
    return { ...view, onArea };
}

beforeEach(() => {
    invalidate(() => true);
    for (const mock of Object.values(api)) mock.mockClear();
    localGit.remotes.mockReset().mockResolvedValue([{ name: "origin", url: remoteUrl }]);
    localGit.overview.mockReset().mockResolvedValue({ status: { branch: "feat/x" }, branches: [], log: [] });
    api.status.mockReset().mockResolvedValue(signedIn);
    api.resolveRemote
        .mockReset()
        .mockResolvedValue({ repo: { host: "example.test", owner: "nodelike", name: "sikemux" }, slug: "nodelike/sikemux", sameHost: true });
    api.accountFor.mockReset().mockResolvedValue("ada-id");
    api.commitAuthors.mockReset().mockResolvedValue([{ email: "Grace@Example.com", login: "grace", avatarUrl: "https://example.test/grace.png" }]);
    hostSettings(TEST_HOST).update((settings) => ({ ...settings, repoByProject: {}, accountByProject: {} }));
    signIn.last = null;
});

afterEach(cleanup);

const openAccountMenu = async () => {
    await userEvent.click(await screen.findByRole("button", { name: "Test host account" }));
    return screen.getByRole("menu");
};

describe("GitHostShell", () => {
    it("shows only the local screens for a folder on no known host", async () => {
        api.resolveRemote.mockResolvedValue({ repo: { host: "gitlab.com", owner: "a", name: "b" }, slug: "a/b", sameHost: false });
        shell("local");
        await waitFor(() => expect(api.resolveRemote).toHaveBeenCalledWith(remoteUrl));
        expect(screen.getByText("local workbench")).toBeTruthy();
        expect(screen.getByRole("button", { name: "Changes" })).toBeTruthy();
        expect(screen.queryByRole("button", { name: "Pull requests" })).toBeNull();
    });

    it("shows only the local screens for a folder with no remote", async () => {
        localGit.remotes.mockResolvedValue([]);
        shell("pulls");
        await waitFor(() => expect(localGit.remotes).toHaveBeenCalledWith(CWD));
        expect(screen.getByText("local workbench")).toBeTruthy();
        expect(api.resolveRemote).not.toHaveBeenCalled();
    });

    it("adds the host's sections to the rail and opens the one pressed", async () => {
        const { onArea } = shell("local");
        await userEvent.click(await screen.findByRole("button", { name: "Issues" }));
        expect(onArea).toHaveBeenCalledWith("issues");
    });

    it("opens a host section on the project's repository, branch and first account that can see it", async () => {
        shell("pulls");
        expect((await screen.findByTestId("host-area")).textContent).toBe(`pulls nodelike/sikemux as ada-id on feat/x in ${CWD}`);
        expect(api.accountFor).toHaveBeenCalledWith({ provider: TEST_HOST, owner: "nodelike", name: "sikemux" });
    });

    it("uses the account picked for the project without asking which can see it", async () => {
        hostSettings(TEST_HOST).update((settings) => ({ ...settings, accountByProject: { [CWD]: "grace-id" } }));
        shell("pulls");
        expect((await screen.findByTestId("host-area")).textContent).toContain("as grace-id");
        expect(api.accountFor).not.toHaveBeenCalled();
    });

    it("shows a repository picked by hand, without the project folder that is not its own", async () => {
        setProjectRepo(TEST_HOST, CWD, "team/fork");
        shell("pulls");
        expect((await screen.findByTestId("host-area")).textContent).toBe("pulls team/fork as ada-id on feat/x in null");
    });

    it("gives local commits the host's pictures once someone is signed in", async () => {
        shell(
            "local",
            <>
                <AuthorAvatar name="Grace Hopper" email="grace@example.com" />
                <AuthorAvatar name="No Reply" email="1+nr@noreply.example" />
                <AuthorAvatar name="Nobody" email="nobody@example.com" />
            </>,
        );
        await waitFor(() => expect(document.querySelectorAll("img")).toHaveLength(2));
        const sources = Array.from(document.querySelectorAll("img")).map((image) => image.getAttribute("src"));
        expect(sources).toEqual(["data:https://example.test/grace.png", "data:https://example.test/noreply.png"]);
        expect(api.commitAuthors).toHaveBeenCalledWith(expect.objectContaining({ owner: "nodelike" }), null);
        expect(document.querySelectorAll("span.gg-avatar")).toHaveLength(1);
    });

    it("leaves local commits with initials while nobody is signed in", async () => {
        api.status.mockResolvedValue({ ...signedIn, ok: false });
        shell("local", <AuthorAvatar name="Grace Hopper" email="grace@example.com" />);
        await screen.findByRole("button", { name: "Sign in to Test host" });
        expect(document.querySelector("img")).toBeNull();
        expect(api.commitAuthors).not.toHaveBeenCalled();
    });

    it("reads the host again after a push or pull in this folder, and not in another", async () => {
        shell("local", <PullCount />);
        await screen.findByText("pulls read: yes");
        expect(api.pulls).toHaveBeenCalledTimes(1);
        act(() => emit({ type: "git-refresh", repo: "/work/other" }));
        await act(async () => {});
        expect(api.pulls).toHaveBeenCalledTimes(1);
        act(() => emit({ type: "git-refresh", repo: CWD }));
        await waitFor(() => expect(api.pulls).toHaveBeenCalledTimes(2));
    });

    it("picks another repository for the project, and forgets the choice when the remote's own is picked", async () => {
        shell("pulls");
        await userEvent.click(within(await openAccountMenu()).getByRole("menuitem", { name: "Choose another repository…" }));
        await userEvent.click(await screen.findByRole("button", { name: "team/fork" }));
        expect(hostSettings(TEST_HOST).get().repoByProject).toEqual({ [CWD]: "team/fork" });
        await userEvent.click(screen.getByRole("button", { name: "nodelike/sikemux" }));
        expect(hostSettings(TEST_HOST).get().repoByProject).toEqual({});
        await userEvent.click(screen.getByRole("button", { name: "Close picker" }));
        expect(screen.queryByRole("dialog", { name: "picker" })).toBeNull();
    });

    it("adds another account and switches the project to it", async () => {
        shell("pulls");
        await userEvent.click(within(await openAccountMenu()).getByRole("menuitem", { name: "Add another account…" }));
        expect(screen.getByText(/Add another Test host account/)).toBeTruthy();
        expect(screen.queryByTestId("host-area")).toBeNull();
        act(() => signIn.last?.("grace-id"));
        expect(hostSettings(TEST_HOST).get().accountByProject).toEqual({ [CWD]: "grace-id" });
        expect(await screen.findByTestId("host-area")).toBeTruthy();
    });

    it("keeps the project's account when adding one is cancelled or ends without one", async () => {
        shell("pulls");
        await userEvent.click(within(await openAccountMenu()).getByRole("menuitem", { name: "Add another account…" }));
        await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
        expect(await screen.findByTestId("host-area")).toBeTruthy();

        await userEvent.click(within(await openAccountMenu()).getByRole("menuitem", { name: "Add another account…" }));
        act(() => signIn.last?.(null));
        expect(hostSettings(TEST_HOST).get().accountByProject).toEqual({});
        expect(await screen.findByTestId("host-area")).toBeTruthy();
    });
});
