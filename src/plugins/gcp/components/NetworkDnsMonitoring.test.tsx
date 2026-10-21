import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({}) as ReturnType<typeof import("../testing").mockApi>);
vi.mock("../api", async (importOriginal) => {
    const { mockApi } = await import("../testing");
    Object.assign(api, mockApi());
    return { ...(await importOriginal<object>()), gcpApi: api };
});
const confirm = vi.hoisted(() => vi.fn(() => Promise.resolve(true)));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), confirmDialog: confirm }));

import { answerEmpty } from "../testing";
import { DnsPage } from "./DnsPage";
import { MonitoringPage } from "./MonitoringPage";
import { NetworkPage, opensPort } from "./NetworkPage";
import { gcp } from "../resources";

const target = { account: null, project: "acme-prod" };
const rule = (name: string, protocols: string[], extra = {}) => ({
    name,
    network: "default",
    direction: "INGRESS" as const,
    priority: 1000,
    action: "allow" as const,
    protocols,
    sources: ["0.0.0.0/0"],
    destinations: [],
    targets: [],
    disabled: false,
    logging: false,
    description: null,
    ...extra,
});

beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    answerEmpty(api);
    api.network.mockResolvedValue({
        networks: [{ name: "default", mode: "auto", routing: "REGIONAL", mtu: 1460, subnets: 40, createdAt: null }],
        subnets: [],
        firewalls: [rule("allow-ssh", ["tcp:22"]), rule("allow-db", ["tcp:5000-6000"]), rule("allow-icmp", ["icmp"])],
    });
    api.firewallToggle.mockResolvedValue({ operation: "op" });
    api.dnsZones.mockResolvedValue([
        {
            name: "acme-dev",
            dnsName: "acme.dev.",
            visibility: "public",
            dnssec: true,
            nameServers: ["ns-cloud-a1.googledomains.com."],
            description: null,
        },
    ]);
    api.dnsRecords.mockResolvedValue([{ name: "shop.acme.dev.", type: "A", ttl: 300, data: ["34.120.1.2"] }]);
    api.alerting.mockResolvedValue({
        policies: [
            {
                id: "123",
                name: "API 5xx rate",
                enabled: true,
                conditions: [{ name: "c", summary: "run.googleapis.com/request_count > 5 for 300s" }],
                combiner: "OR",
                channels: 0,
                documentation: null,
            },
        ],
        uptimeChecks: [],
        channels: [],
    });
    api.alertPolicyToggle.mockResolvedValue({});
});

afterEach(() => {
    cleanup();
    gcp.refresh();
});

describe("VPC network", () => {
    it("knows which rules let a port through", () => {
        expect(opensPort(rule("a", ["tcp:22"]), "22")).toBe(true);
        expect(opensPort(rule("b", ["tcp:5000-6000"]), "5432")).toBe(true);
        expect(opensPort(rule("c", ["icmp"]), "5432")).toBe(false);
        expect(opensPort(rule("d", ["tcp"]), "8080")).toBe(true);
        expect(opensPort(rule("e", ["tcp:22"]), "ssh")).toBe(false);
    });

    it("filters firewall rules by port and turns one off once confirmed", async () => {
        render(<NetworkPage paneId="net-1" active target={target} />);
        fireEvent.change(await screen.findByRole("textbox", { name: "Filter rules, or a port such as 5432" }), { target: { value: "5432" } });
        const table = screen.getByRole("table", { name: "Firewall rules" });
        expect(within(table).getByText("allow-db")).toBeInTheDocument();
        expect(within(table).queryByText("allow-ssh")).not.toBeInTheDocument();
        fireEvent.click(within(table).getByRole("button", { name: "Turn off" }));
        await waitFor(() => expect(api.firewallToggle).toHaveBeenCalledWith(target, "allow-db", true));
    });
});

describe("Cloud DNS", () => {
    it("opens a zone to its records", async () => {
        render(<DnsPage paneId="dns-1" active target={target} />);
        fireEvent.click(await screen.findByRole("row", { name: /acme-dev/ }));
        expect(await screen.findByText("34.120.1.2")).toBeInTheDocument();
    });
});

describe("Monitoring", () => {
    it("says when an alert notifies no one, and turns one off once confirmed", async () => {
        render(<MonitoringPage paneId="mon-1" active target={target} />);
        expect(await screen.findByText("No one")).toBeInTheDocument();
        expect(screen.getByText("run.googleapis.com/request_count > 5 for 300s")).toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: "Turn off" }));
        await waitFor(() => expect(api.alertPolicyToggle).toHaveBeenCalledWith(target, "123", false));
    });
});
