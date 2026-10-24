import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { VercelDeployment, VercelLog, VercelProject, VercelStatus } from "../api";

const api = vi.hoisted(() => ({
    status: vi.fn(),
    signIn: vi.fn(),
    signOut: vi.fn(),
    setTeam: vi.fn(),
    redeploy: vi.fn(),
    promote: vi.fn(),
    cancel: vi.fn(),
}));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), vercelApi: api }));
const confirmDialog = vi.hoisted(() => vi.fn());
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), confirmDialog }));

const views = vi.hoisted(() => ({
    projectsView: vi.fn(),
    deploymentsView: vi.fn(),
    logView: vi.fn(),
    domainsView: vi.fn(),
    envView: vi.fn(),
}));

import { fakeViews } from "../../../plugin-api/fakeViews";
import { VercelPane } from "./VercelPane";
import { vercel } from "../resources";

fakeViews(views);

const signedIn: VercelStatus = {
    configured: true,
    ok: true,
    authFailed: false,
    message: null,
    account: { id: "u1", username: "ankit", name: "Ankit", email: "a@example.com", signIn: "token" },
    teams: [{ id: "team_1", slug: "acme", name: "Acme" }],
    team: "team_1",
    personal: true,
    browserSignIn: true,
};

const project: VercelProject = {
    id: "prj_1",
    name: "web",
    framework: "nextjs",
    updatedAt: Date.now() - 60_000,
    repo: "acme/web",
    gitProvider: "github",
    productionBranch: "main",
    production: { id: "dpl_live", url: "https://web.acme.dev", state: "READY", createdAt: Date.now() - 3_600_000 },
    latestProduction: null,
};

const failed: VercelDeployment = {
    id: "dpl_bad",
    project: "web",
    projectId: "prj_1",
    url: "https://web-bad.vercel.app",
    state: "ERROR",
    target: "preview",
    createdAt: Date.now() - 120_000,
    readyAt: null,
    buildMs: 45_000,
    branch: "fix-nav",
    commit: "abc1234def",
    message: "Fix the nav",
    author: "Ankit",
    redeployOf: null,
    inspectorUrl: "https://vercel.com/acme/web/bad",
    aliases: null,
    error: 'Command "npm run build" exited with 1',
};

const live: VercelDeployment = {
    ...failed,
    id: "dpl_live",
    state: "READY",
    target: "production",
    branch: "main",
    message: "Ship the redesign",
    error: null,
};
const before: VercelDeployment = { ...live, id: "dpl_before", message: "Last week's build", createdAt: Date.now() - 86_400_000 };
const redeploy: VercelDeployment = { ...live, id: "dpl_again", state: "CANCELED", message: "Ship the redesign", redeployOf: "dpl_live123456789" };

const log: VercelLog = {
    deployment: failed,
    lines: [
        { at: 1, kind: "command", text: "npm run build" },
        { at: 2, kind: "stderr", text: "Type error: nav is not defined" },
        { at: 3, kind: "stderr", text: "Build Completed in /vercel/output [41s]" },
    ],
    skipped: 0,
};

let pane = 0;
const renderPane = () => render(<VercelPane paneId={`vercel-${++pane}`} active />);
const openProject = async () => fireEvent.click(await screen.findByRole("button", { name: /web\.acme\.dev.*acme\/web/ }));
const nav = (name: string) => fireEvent.click(within(screen.getByRole("navigation", { name: "Vercel" })).getByRole("button", { name }));
const accept = (title: string) => waitFor(() => expect(confirmDialog).toHaveBeenCalledWith(expect.objectContaining({ title })));

afterEach(() => {
    cleanup();
    vercel.refresh();
});

beforeEach(() => {
    [...Object.values(api), ...Object.values(views)].forEach((mock) => mock.mockReset());
    confirmDialog.mockReset().mockResolvedValue(true);
    api.status.mockResolvedValue(signedIn);
    views.projectsView.mockReturnValue({ projects: [project] });
    views.deploymentsView.mockReturnValue({ deployments: [failed, redeploy, live, before] });
    views.logView.mockImplementation(({ id }: { id: string }) => ({
        log: { ...log, deployment: [failed, redeploy, live, before].find((each) => each.id === id) },
        forAgent: `Vercel deployment ${id}`,
    }));
    views.domainsView.mockReturnValue({ domains: [{ name: "web.acme.dev", verified: true, redirect: null, gitBranch: null, createdAt: null }] });
    views.envView.mockReturnValue({
        variables: [
            { id: "e1", key: "DATABASE_URL", target: ["production", "preview", "development"], type: "encrypted", gitBranch: null, updatedAt: 1 },
        ],
    });
    api.promote.mockResolvedValue(before);
    api.redeploy.mockResolvedValue(failed);
    api.setTeam.mockResolvedValue(undefined);
});

describe("the Vercel pane", () => {
    it("offers the browser sign-in, or an access token, when no account is signed in", async () => {
        api.status.mockResolvedValue({ ...signedIn, configured: false, account: null, teams: [], team: null });
        renderPane();
        expect(await screen.findByRole("heading", { name: "Connect Vercel" })).toBeInTheDocument();
        expect(screen.getByRole("button", { name: "Continue with Vercel" })).toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: "Use an access token instead" }));
        fireEvent.change(screen.getByPlaceholderText("Paste a Vercel token"), { target: { value: "tok_123" } });
        api.signIn.mockResolvedValue(signedIn);
        fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
        await waitFor(() => expect(api.signIn).toHaveBeenCalledWith("tok_123"));
    });

    it("opens a project on its production deployment, as Vercel's overview does", async () => {
        renderPane();
        await openProject();
        expect(views.projectsView).toHaveBeenCalledWith({ team: "team_1" });
        const production = await screen.findByRole("region", { name: "Production deployment" });
        expect(within(production).getByText("Ship the redesign")).toBeInTheDocument();
        expect(await within(production).findByRole("button", { name: /web\.acme\.dev/ })).toBeInTheDocument();
        expect(screen.getByText("Active Branches")).toBeInTheDocument();
        expect(screen.getByRole("button", { name: /fix-nav.*Fix the nav/ })).toBeInTheDocument();
    });

    it("rolls production back to the build before it once the person confirms", async () => {
        renderPane();
        await openProject();
        await waitFor(() => expect(screen.getByRole("button", { name: /Instant Rollback/ })).toBeEnabled());
        fireEvent.click(screen.getByRole("button", { name: /Instant Rollback/ }));
        await accept("Roll back production?");
        await waitFor(() => expect(api.promote).toHaveBeenCalledWith("team_1", "dpl_before"));
    });

    it("lists deployments as Vercel does, filters them and acts from each row's menu", async () => {
        renderPane();
        await openProject();
        await screen.findByRole("region", { name: "Production deployment" });
        nav("Deployments");
        const table = await screen.findByRole("table", { name: "Deployments" });
        expect(within(table).getAllByText("45s")).toHaveLength(4);
        expect(within(table).getByText("Redeploy of")).toBeInTheDocument();
        expect(within(table).getByTitle("Serving production now")).toHaveTextContent("Production");

        fireEvent.click(within(table).getByRole("button", { name: "Actions for Fix the nav" }));
        expect(screen.getByRole("menuitem", { name: /Instant Rollback/ })).toBeDisabled();
        fireEvent.click(screen.getByRole("menuitem", { name: "Redeploy" }));
        await accept("Redeploy?");
        await waitFor(() => expect(api.redeploy).toHaveBeenCalledWith("team_1", "dpl_bad"));
    });

    it("shows a failed deployment's error and build log, marking only real errors", async () => {
        renderPane();
        await openProject();
        await screen.findByRole("region", { name: "Production deployment" });
        nav("Deployments");
        fireEvent.click(await screen.findByRole("button", { name: "Open Fix the nav" }));
        const error = await screen.findByText("Type error: nav is not defined");
        expect(error.closest(".vercel-log-line")).toHaveClass("error");
        expect(screen.getByText("Build Completed in /vercel/output [41s]").closest(".vercel-log-line")).not.toHaveClass("error");
        expect(screen.getByText('Command "npm run build" exited with 1')).toBeInTheDocument();
    });

    it("says when a newer production build failed while the one before it still serves", async () => {
        views.projectsView.mockReturnValue({
            projects: [{ ...project, latestProduction: { id: "dpl_bad", url: null, state: "ERROR", createdAt: Date.now() - 120_000 } }],
        });
        renderPane();
        await openProject();
        fireEvent.click(await screen.findByRole("button", { name: /latest production build is error/ }));
        expect(await screen.findByText("Type error: nav is not defined")).toBeInTheDocument();
    });

    it("lists domains, and environment variable names without values", async () => {
        renderPane();
        await openProject();
        await screen.findByRole("region", { name: "Production deployment" });
        nav("Domains");
        expect(await screen.findByText("Valid Configuration")).toBeInTheDocument();
        nav("Environment Variables");
        expect(await screen.findByText("DATABASE_URL")).toBeInTheDocument();
        expect(screen.getByText("All Environments")).toBeInTheDocument();
        expect(screen.getByText(/Values stay in Vercel/)).toBeInTheDocument();
    });

    it("lets the person resize the sidebar", async () => {
        renderPane();
        const handle = await screen.findByRole("separator", { name: "Resize the Vercel sidebar" });
        fireEvent.keyDown(handle, { key: "ArrowRight" });
        await waitFor(() => expect(screen.getByRole("complementary")).toHaveStyle({ width: "180px" }));
    });
});
