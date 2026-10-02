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

    it("marks the open section and opens the one pressed", async () => {
        const props = handlers();
        renderRail(props);
        expect(screen.getByRole("button", { name: "Pull requests" }).getAttribute("aria-current")).toBe("page");
        expect(screen.getByRole("button", { name: "Issues" }).getAttribute("aria-current")).toBeNull();
        await userEvent.click(screen.getByRole("button", { name: "Releases" }));
        expect(props.onArea).toHaveBeenCalledWith("releases");
    });
});

describe("GitRail", () => {
    it("draws the local screens with their counts, capping a big one", async () => {
        const onSelect = vi.fn();
        render(
            <GitRail
                local={[
                    { id: "changes", label: "Changes", icon: null, count: 3, on: true, onSelect },
                    { id: "history", label: "History", icon: null, count: 250, on: false, onSelect: vi.fn() },
                    { id: "stash", label: "Stash", icon: null, count: 0, on: false, onSelect: vi.fn() },
                ]}
                host={null}
            />,
        );
        expect(screen.getByRole("button", { name: "Changes" }).textContent).toBe("3");
        expect(screen.getByRole("button", { name: "History" }).textContent).toBe("99+");
        expect(screen.getByRole("button", { name: "Stash" }).textContent).toBe("");
        await userEvent.click(screen.getByRole("button", { name: "Changes" }));
        expect(onSelect).toHaveBeenCalled();
    });
});

describe("the account at the foot of the rail", () => {
    it("offers to sign in while nobody is, which opens the pull requests", async () => {
        api.status.mockResolvedValue({ ...signedIn, ok: false, login: "" });
        const props = handlers();
        renderRail(props);
        await userEvent.click(await screen.findByRole("button", { name: "Sign in to Test host" }));
        expect(props.onArea).toHaveBeenCalledWith("pulls");
    });

    it("asks the host about the account the project uses", async () => {
        renderRail(handlers(), { account: "grace-id" });
        await screen.findByRole("button", { name: "Test host account" });
        expect(api.status).toHaveBeenCalledWith("grace-id");
    });

    it("shows the account's picture when it has one", async () => {
        api.status.mockResolvedValue({ ...signedIn, avatarUrl: "https://example.test/ada.png" });
        renderRail(handlers());
        const button = await screen.findByRole("button", { name: "Test host account" });
        await waitFor(() => expect(button.querySelector("img")?.getAttribute("src")).toBe("data:image/png;base64,AA=="));
        expect(api.image).toHaveBeenCalledWith("https://example.test/ada.png");
    });

    it("says who is signed in where, and warns about what the token lacks", async () => {
        api.status.mockResolvedValue({ ...signedIn, warning: "This token cannot start workflows." });
        renderRail(handlers());
        const menu = await openMenu();
        expect(menu.textContent).toContain("ada on example.test");
        expect(within(menu).getByText("nodelike/sikemux")).toBeTruthy();
        expect(within(menu).getByText("This token cannot start workflows.")).toBeTruthy();
    });

    it("lists no accounts to switch between, and no default to change, while there is only one", async () => {
        renderRail(handlers(), { slug: null });
        const menu = await openMenu();
        await waitFor(() => expect(api.accounts).toHaveBeenCalled());
        expect(within(menu).queryAllByRole("menuitemradio")).toHaveLength(0);
        expect(within(menu).queryByText(/Open new projects as/)).toBeNull();
        expect(menu.querySelector(".git-rail-repo")).toBeNull();
    });

    it("hands off to add an account or choose a repository, closing the menu", async () => {
        const props = handlers();
        renderRail(props);
        await userEvent.click(within(await openMenu()).getByRole("menuitem", { name: "Add another account…" }));
        expect(props.onAddAccount).toHaveBeenCalled();
        expect(screen.queryByRole("menu")).toBeNull();

        await userEvent.click(within(await openMenu()).getByRole("menuitem", { name: "Choose another repository…" }));
        expect(props.onPickRepo).toHaveBeenCalled();
        expect(screen.queryByRole("menu")).toBeNull();
    });

    it("closes when clicked away from", async () => {
        const { container } = renderRail(handlers());
        await openMenu();
        await userEvent.click(container.ownerDocument.querySelector(".env-dd-scrim")!);
        expect(screen.queryByRole("menu")).toBeNull();
    });

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

    it("makes the account in use the default for new projects", async () => {
        api.accounts.mockResolvedValue([entry("grace-id", "grace", { isDefault: true }), entry("ada-id", "ada")]);
        renderRail(handlers());
        const menu = await openMenu();
        await userEvent.click(await within(menu).findByRole("menuitem", { name: "Open new projects as ada" }));
        expect(api.setDefaultAccount).toHaveBeenCalledWith("ada-id");
        await waitFor(() => expect(toasts()).toContain("success: New projects open as ada"));
    });

    it("reports a default that could not be changed", async () => {
        api.accounts.mockResolvedValue([entry("grace-id", "grace", { isDefault: true }), entry("ada-id", "ada")]);
        api.setDefaultAccount.mockRejectedValue(new Error("offline"));
        renderRail(handlers());
        await userEvent.click(await within(await openMenu()).findByRole("menuitem", { name: "Open new projects as ada" }));
        await waitFor(() => expect(toasts()).toContain("error: Could not change the default account: offline"));
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

    it("reports a sign-out that failed", async () => {
        api.signOut.mockRejectedValue({ category: "http", message: "http 500" });
        renderRail(handlers());
        await userEvent.click(within(await openMenu()).getByRole("menuitem", { name: /Sign ada out/ }));
        await waitFor(() => expect(toasts()).toContain("error: Could not sign out: http 500"));
    });
});
