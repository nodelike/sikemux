import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { HostAccount, HostAccountEntry } from "../registry";

const api = vi.hoisted(() => ({
    status: vi.fn(),
    accounts: vi.fn(),
    signOut: vi.fn(),
    setDefaultAccount: vi.fn(),
    image: vi.fn(() => Promise.resolve("data:image/png;base64,AA==")),
}));

import { invalidate } from "../../plugin-api/resources";
import { useToasts } from "../../state/toast";
import { AccountProvider, type CodeHost } from "../registry";
import { hostSettings, setProjectAccount } from "../state";
import { InHost, registerTestHost, TEST_HOST } from "../testHost";
import { GitRail, HostRailItems, sectionLabel, sectionsOf } from "./HostRail";

const host = registerTestHost(api);
const CWD = "/work/sikemux";

const signedIn: HostAccount = {
    id: "ada-id",
    ok: true,
    login: "ada",
    avatarUrl: null,
    host: "example.test",
    canWriteCi: true,
    warning: null,
};

const entry = (id: string, login: string, extra: Partial<HostAccountEntry> = {}): HostAccountEntry => ({
    id,
    login,
    detail: null,
    avatarUrl: null,
    isDefault: false,
    ...extra,
});

const handlers = () => ({ onArea: vi.fn(), onPickRepo: vi.fn(), onAddAccount: vi.fn() });

function renderRail(props: ReturnType<typeof handlers>, { account = null as string | null, slug = "nodelike/sikemux" as string | null } = {}) {
    return render(
        <InHost host={host}>
            <AccountProvider value={account}>
                <GitRail local={[]} host={<HostRailItems area="pulls" slug={slug} cwd={CWD} active {...props} />} />
            </AccountProvider>
        </InHost>,
    );
}

const openMenu = async () => {
    await userEvent.click(await screen.findByRole("button", { name: "Test host account" }));
    return screen.getByRole("menu");
};

beforeEach(() => {
    invalidate(() => true);
    for (const mock of Object.values(api)) mock.mockReset();
    api.image.mockResolvedValue("data:image/png;base64,AA==");
    api.status.mockResolvedValue(signedIn);
    api.accounts.mockResolvedValue([entry("ada-id", "ada", { isDefault: true })]);
    api.signOut.mockResolvedValue(undefined);
    api.setDefaultAccount.mockResolvedValue(undefined);
    hostSettings(TEST_HOST).update((settings) => ({ ...settings, accountByProject: {} }));
    useToasts.setState({ toasts: [] });
});

afterEach(cleanup);

const toasts = () => useToasts.getState().toasts.map((toast) => `${toast.kind}: ${toast.text}`);

describe("sections", () => {
    it("leaves out what the host cannot do, and names its CI its own way", () => {
        const plain = {
            ...host,
            ciName: "Pipelines",
            capabilities: { ...host.capabilities, issues: false, releases: false, inbox: false },
        } as CodeHost;
        expect(sectionsOf(plain)).toEqual(["pulls", "actions"]);
        expect(sectionsOf(plain).map((section) => sectionLabel(plain, section))).toEqual(["Pull requests", "Pipelines"]);
        expect(sectionsOf(host).map((section) => sectionLabel(host, section))).toEqual(["Pull requests", "CI", "Issues", "Releases", "Inbox"]);
    });
});

describe("the account at the foot of the rail", () => {
    it("switches this project to another signed-in account", async () => {
        api.accounts.mockResolvedValue([
            entry("ada-id", "ada", { isDefault: true }),
            entry("grace-id", "grace", { detail: "git.corp.example", avatarUrl: "https://example.test/grace.png" }),
        ]);
        renderRail(handlers());
        const menu = await openMenu();
        const rows = await within(menu).findAllByRole("menuitemradio");
        expect(rows.map((row) => row.getAttribute("aria-checked"))).toEqual(["true", "false"]);
        expect(rows[1]?.textContent).toContain("git.corp.example");
        expect(within(menu).queryByText(/Open new projects as/)).toBeNull();
        await userEvent.click(rows[1]!);
        expect(hostSettings(TEST_HOST).get().accountByProject[CWD]).toBe("grace-id");
    });

    it("signs the account out and makes every project that picked it find another", async () => {
        setProjectAccount(TEST_HOST, CWD, "ada-id");
        setProjectAccount(TEST_HOST, "/work/other", "grace-id");
        renderRail(handlers(), { account: "ada-id" });
        await userEvent.click(within(await openMenu()).getByRole("menuitem", { name: /Sign ada out/ }));
        expect(api.signOut).toHaveBeenCalledWith("ada-id");
        await waitFor(() => expect(toasts()).toContain("success: Signed ada out of Test host"));
        expect(hostSettings(TEST_HOST).get().accountByProject).toEqual({ "/work/other": "grace-id" });
    });

    it("signs out a borrowed token without forgetting any project's account", async () => {
        api.status.mockResolvedValue({ ...signedIn, id: null });
        setProjectAccount(TEST_HOST, "/work/other", "grace-id");
        renderRail(handlers());
        await userEvent.click(within(await openMenu()).getByRole("menuitem", { name: /Sign ada out/ }));
        expect(api.signOut).toHaveBeenCalledWith(null);
        await waitFor(() => expect(toasts()).toContain("success: Signed ada out of Test host"));
        expect(hostSettings(TEST_HOST).get().accountByProject).toEqual({ "/work/other": "grace-id" });
    });
});
