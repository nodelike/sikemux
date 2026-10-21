import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { API, issue, signedIn, signedOut, TYPE_ERROR, TYPE_ERROR_DETAIL, WEB } from "../testing";

const api = vi.hoisted(() => ({
    status: vi.fn(),
    signIn: vi.fn(),
    signOut: vi.fn(),
    organizations: vi.fn(),
    choose: vi.fn(),
    projects: vi.fn(),
    members: vi.fn(),
    environments: vi.fn(),
    issues: vi.fn(),
    issue: vi.fn(),
    brief: vi.fn(),
    change: vi.fn(),
    comment: vi.fn(),
    releases: vi.fn(),
}));
const host = vi.hoisted(() => ({ notify: vi.fn(), openProjectFile: vi.fn(), pathKinds: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), sentryApi: api }));
vi.mock("../../../plugin-api/host", async (importOriginal) => {
    const real = await importOriginal<typeof import("../../../plugin-api/host")>();
    return {
        ...real,
        notify: host.notify,
        openProjectFile: host.openProjectFile,
        useNearestProject: () => ({ name: "shop", cwd: "/Users/me/shop" }),
        files: { ...real.files, pathKinds: host.pathKinds },
    };
});

import { sentry } from "../resources";
import { SentryPane } from "./SentryPane";

let pane = 0;
const renderPane = () => render(<SentryPane paneId={`sentry-${++pane}`} active />);

afterEach(() => {
    cleanup();
    sentry.refresh();
});
beforeEach(() => {
    Object.values({ ...api, ...host }).forEach((mock) => mock.mockReset());
    api.status.mockResolvedValue(signedIn);
    api.organizations.mockResolvedValue([{ slug: "acme", name: "Acme" }]);
    api.projects.mockResolvedValue([API, WEB]);
    api.members.mockResolvedValue([{ actor: "user:42", name: "Ankit", email: "ankit@acme.dev", isMe: true }]);
    api.environments.mockResolvedValue(["production", "staging"]);
    api.issues.mockResolvedValue({ issues: [TYPE_ERROR, issue("2Z", { level: "warning" })], next: null });
    api.issue.mockResolvedValue(TYPE_ERROR_DETAIL);
    api.change.mockResolvedValue({ ...TYPE_ERROR, status: "resolved" });
    api.choose.mockResolvedValue(signedIn);
});

describe("the Sentry pane", () => {
    it("asks for a personal token when nothing is signed in", async () => {
        api.status.mockResolvedValue(signedOut);
        renderPane();
        expect(await screen.findByText("Connect Sentry")).toBeInTheDocument();
        expect(screen.getByText(/User Settings → Personal Tokens/)).toBeInTheDocument();
    });

    it("lists unresolved issues across projects, and narrows to one project", async () => {
        renderPane();
        const list = await screen.findByRole("list", { name: "Issues" });
        expect(within(list).getByRole("button", { name: /WEB-3F/ })).toHaveTextContent("1.3k");
        expect(within(list).getByRole("button", { name: /WEB-3F/ })).toHaveTextContent("Regressed");
        expect(api.issues).toHaveBeenCalledWith(expect.objectContaining({ projects: [], status: "unresolved", sort: "date", period: "24h" }));
        fireEvent.click(screen.getByRole("button", { name: /Project/ }));
        fireEvent.click(await screen.findByRole("option", { name: /^web/ }));
        await waitFor(() => expect(api.issues).toHaveBeenLastCalledWith(expect.objectContaining({ projects: ["11"] })));
        expect(api.choose).toHaveBeenCalledWith({ project: "web" });
        fireEvent.click(within(screen.getByRole("group", { name: "Sort" })).getByRole("button", { name: "Events" }));
        await waitFor(() => expect(api.issues).toHaveBeenLastCalledWith(expect.objectContaining({ sort: "freq" })));
    });

    it("opens an issue at the frame that threw, and opens that line in the project", async () => {
        host.pathKinds.mockResolvedValue(["file", null, null]);
        renderPane();
        fireEvent.click(await screen.findByRole("button", { name: /WEB-3F/ }));
        const thrown = await screen.findByRole("article", { name: /TypeError: Cannot read properties/ });
        expect(within(thrown).getByText("return lines.total;", { exact: false })).toBeInTheDocument();
        expect(within(thrown).getByText("2 library frames")).toBeInTheDocument();
        fireEvent.click(within(thrown).getByRole("button", { name: "Open app/checkout/summary.ts:12:22" }));
        await waitFor(() => expect(host.openProjectFile).toHaveBeenCalledWith("/Users/me/shop/app/checkout/summary.ts", 12));
        expect(host.pathKinds).toHaveBeenCalledWith([
            "/Users/me/shop/app/checkout/summary.ts",
            "/Users/me/shop/checkout/summary.ts",
            "/Users/me/shop/summary.ts",
        ]);
    });

    it("says when a frame's file is not in the project", async () => {
        host.pathKinds.mockResolvedValue([null, null, null]);
        renderPane();
        fireEvent.click(await screen.findByRole("button", { name: /WEB-3F/ }));
        fireEvent.click(await screen.findByRole("button", { name: "Open app/checkout/summary.ts:12:22" }));
        await waitFor(() => expect(host.notify).toHaveBeenCalledWith("info", "summary.ts isn't in shop."));
        expect(host.openProjectFile).not.toHaveBeenCalled();
    });

    it("resolves an issue and shows its breadcrumbs", async () => {
        renderPane();
        fireEvent.click(await screen.findByRole("button", { name: /WEB-3F/ }));
        fireEvent.click(await screen.findByRole("button", { name: "Resolve" }));
        await waitFor(() => expect(api.change).toHaveBeenCalledWith("3F", { action: "resolve" }));
        expect(host.notify).toHaveBeenCalledWith("success", "Resolved");
        fireEvent.click(within(screen.getByRole("group", { name: "Issue view" })).getByRole("button", { name: /Breadcrumbs/ }));
        expect(screen.getByRole("list", { name: "Breadcrumbs" })).toHaveTextContent("GET /api/orders/draft [404]");
    });

    it("shows a project's releases with how many sessions crashed", async () => {
        api.status.mockResolvedValue({ ...signedIn, project: "web" });
        api.releases.mockResolvedValue([
            {
                version: "web@4.13.0",
                short: "4.13.0",
                created: null,
                newIssues: 2,
                commits: 14,
                deployedTo: "production",
                deployedAt: null,
                crashFreeSessions: 97.42,
                crashFreeUsers: 98.1,
                sessions: 18_233,
            },
        ]);
        renderPane();
        fireEvent.click(within(await screen.findByRole("navigation", { name: "Sentry" })).getByRole("button", { name: "Releases" }));
        const table = await screen.findByRole("table", { name: "Releases" });
        expect(api.releases).toHaveBeenCalledWith(WEB);
        expect(within(table).getByText("97.42%").closest(".sentry-rate")).toHaveClass("danger");
        fireEvent.click(within(table).getByRole("button", { name: "2" }));
        await waitFor(() =>
            expect(api.issues).toHaveBeenLastCalledWith(expect.objectContaining({ query: "firstRelease:web@4.13.0", status: "all" })),
        );
    });
});
