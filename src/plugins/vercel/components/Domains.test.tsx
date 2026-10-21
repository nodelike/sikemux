import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DomainDetail } from "../api";

const api = vi.hoisted(() => ({
    domains: vi.fn(),
    domain: vi.fn(),
    addDomain: vi.fn(),
    updateDomain: vi.fn(),
    verifyDomain: vi.fn(),
    removeDomain: vi.fn(),
}));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), vercelApi: api }));
const host = vi.hoisted(() => ({ confirmDialog: vi.fn(), notify: vi.fn(), copyWithNotice: vi.fn() }));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), ...host }));

import { vercel } from "../resources";
import { Domains, routingOf } from "./Domains";

const pending: DomainDetail = {
    name: "shop.acme.dev",
    verified: false,
    misconfigured: true,
    redirect: null,
    redirectStatusCode: null,
    gitBranch: null,
    records: [
        { type: "TXT", name: "_vercel.acme.dev", value: "vc-domain-verify=shop.acme.dev,abc", why: "Proves the domain is yours" },
        { type: "CNAME", name: "shop", value: "cname.vercel-dns.com", why: "Points the domain at Vercel" },
    ],
};

afterEach(() => {
    cleanup();
    vercel.refresh();
});
beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    Object.values(host).forEach((mock) => mock.mockReset());
    api.domains.mockResolvedValue([{ name: "shop.acme.dev", verified: false, redirect: null, gitBranch: null }]);
    api.domain.mockResolvedValue(pending);
    api.verifyDomain.mockResolvedValue({ ...pending, verified: true, misconfigured: false, records: [] });
    api.addDomain.mockResolvedValue(pending);
    api.removeDomain.mockResolvedValue(undefined);
    host.confirmDialog.mockResolvedValue(true);
});

describe("a project's domains", () => {
    it("routes to production, a branch or a redirect, never both", () => {
        expect(routingOf("branch", " staging ", "acme.dev")).toEqual({ branch: "staging", redirect: "" });
        expect(routingOf("redirect", "staging", " acme.dev ")).toEqual({ redirect: "acme.dev", branch: "" });
        expect(routingOf("production", "x", "y")).toEqual({ branch: "", redirect: "" });
    });

    it("lists the DNS records a domain still needs, copies a value, and checks again", async () => {
        render(<Domains active team="team_acme" project="web" />);
        const records = await screen.findByRole("table", { name: "DNS records for shop.acme.dev" });
        expect(within(records).getAllByRole("row")).toHaveLength(3);
        fireEvent.click(within(records).getByRole("button", { name: "cname.vercel-dns.com" }));
        expect(host.copyWithNotice).toHaveBeenCalledWith("cname.vercel-dns.com", "CNAME value");
        expect(screen.getByText("Verification Needed")).toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
        await waitFor(() => expect(host.notify).toHaveBeenCalledWith("success", "shop.acme.dev is set up"));
    });

    it("adds a domain for a branch", async () => {
        render(<Domains active team="team_acme" project="web" />);
        fireEvent.click(await screen.findByRole("button", { name: "Add Domain" }));
        const form = screen.getByRole("form", { name: "Add Domain" });
        fireEvent.change(within(form).getByRole("textbox", { name: "Domain" }), { target: { value: "staging.acme.dev" } });
        fireEvent.click(within(form).getByRole("button", { name: "A branch" }));
        expect(within(form).getByRole("button", { name: "Add" })).toBeDisabled();
        fireEvent.change(within(form).getByRole("textbox", { name: "Git branch" }), { target: { value: "staging" } });
        fireEvent.click(within(form).getByRole("button", { name: "Add" }));
        await waitFor(() => expect(api.addDomain).toHaveBeenCalledWith("team_acme", "web", "staging.acme.dev", { branch: "staging", redirect: "" }));
    });

    it("removes a domain after saying its DNS stays", async () => {
        render(<Domains active team="team_acme" project="web" />);
        const card = await screen.findByRole("region", { name: "shop.acme.dev" });
        fireEvent.click(within(card).getByRole("button", { name: "Remove" }));
        await waitFor(() => expect(api.removeDomain).toHaveBeenCalledWith("team_acme", "web", "shop.acme.dev"));
        expect(host.confirmDialog).toHaveBeenCalledWith(expect.objectContaining({ body: expect.stringContaining("Its DNS records stay") }));
    });
});
