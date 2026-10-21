import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { VercelFirewall } from "../api";

const api = vi.hoisted(() => ({ firewall: vi.fn(), setAttackMode: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), vercelApi: api }));
const host = vi.hoisted(() => ({ confirmDialog: vi.fn(), notify: vi.fn() }));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), ...host }));

import { Firewall } from "./Firewall";
import { vercel } from "../resources";

const calm: VercelFirewall = {
    enabled: true,
    rules: [
        { id: "rule_1", name: "Block wp-admin", description: "Bots probing WordPress", active: true, action: "Deny" },
        { id: "rule_2", name: "Slow the API", description: null, active: false, action: "Rate limit" },
    ],
    blocks: [{ ip: "203.0.113.7", hostname: null, note: "scraper", action: "deny" }],
    attackMode: false,
    attackModeUntil: null,
};

afterEach(() => {
    cleanup();
    vercel.refresh();
});
beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    Object.values(host).forEach((mock) => mock.mockReset());
    api.firewall.mockResolvedValue(calm);
    api.setAttackMode.mockResolvedValue({ ...calm, attackMode: true });
    host.confirmDialog.mockResolvedValue(true);
});

describe("the firewall", () => {
    it("lists its rules with what each does, and its blocked addresses", async () => {
        render(<Firewall active team="team_acme" project="web" />);
        const rules = await screen.findByRole("table", { name: "Firewall rules" });
        expect(within(within(rules).getByRole("row", { name: "Block wp-admin" })).getByText("Deny")).toBeInTheDocument();
        expect(within(within(rules).getByRole("row", { name: "Slow the API" })).getByText("Paused")).toBeInTheDocument();
        expect(screen.getByText("203.0.113.7")).toBeInTheDocument();
    });

    it("turns Attack Challenge Mode on for the hours picked, after asking", async () => {
        render(<Firewall active team="team_acme" project="web" />);
        const box = await screen.findByRole("region", { name: "Attack Challenge Mode" });
        fireEvent.click(within(box).getByRole("button", { name: "6 hours" }));
        fireEvent.click(within(box).getByRole("button", { name: "Turn on" }));
        await waitFor(() => expect(api.setAttackMode).toHaveBeenCalledWith("team_acme", "web", true, 6));
        expect(host.confirmDialog).toHaveBeenCalledWith(
            expect.objectContaining({ body: expect.stringMatching(/^For 6 hours, every visitor to web/) }),
        );
    });

    it("turns it off without asking", async () => {
        api.firewall.mockResolvedValue({ ...calm, attackMode: true });
        render(<Firewall active team="team_acme" project="web" />);
        fireEvent.click(await screen.findByRole("button", { name: "Turn off" }));
        await waitFor(() => expect(api.setAttackMode).toHaveBeenCalledWith("team_acme", "web", false, undefined));
        expect(host.confirmDialog).not.toHaveBeenCalled();
    });
});
