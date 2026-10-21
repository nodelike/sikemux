import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({}) as ReturnType<typeof import("../testing").mockApi>);
vi.mock("../api", async (importOriginal) => {
    const { mockApi } = await import("../testing");
    Object.assign(api, mockApi());
    return { ...(await importOriginal<object>()), gcpApi: api };
});
vi.mock("../../../plugin-api/host", async (importOriginal) => ({
    ...(await importOriginal<object>()),
    useNearestProject: () => ({ name: "shop", cwd: "/code/shop" }),
}));

import { answerEmpty } from "../testing";
import { gcpSettings } from "../state";
import { AgentSetup, describePick } from "./AgentSetup";
import { gcp } from "../resources";

beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    answerEmpty(api);
    api.projects.mockResolvedValue([
        { id: "acme-prod", name: "Acme Prod", number: "1", createdAt: "" },
        { id: "acme-dev", name: "Acme Dev", number: "2", createdAt: "" },
    ]);
    api.pickForAgents.mockResolvedValue({ project: null, default: null, effective: null });
    gcpSettings.update(() => ({ account: "ankit@example.com", project: "acme-prod", sideWidth: null }));
});

afterEach(() => {
    cleanup();
    gcp.refresh();
});

describe("what agents start on", () => {
    it("says where a pick comes from", () => {
        expect(describePick(undefined)).toBe("gcloud's own configuration");
        expect(describePick({ project: null, default: null, effective: { pick: { account: "a@x.com", project: "p" }, source: "default" } })).toBe(
            "p as a@x.com",
        );
        expect(describePick({ project: null, default: null, effective: { pick: { account: null, project: "mine" }, source: "environment" } })).toBe(
            "mine, from your shell",
        );
    });

    it("picks a project, as the console's account, for agents in this Sikemux project", async () => {
        render(<AgentSetup />);
        fireEvent.click(await screen.findByRole("button", { name: "Google Cloud project for agents in shop" }));
        fireEvent.click(await screen.findByRole("option", { name: "acme-dev" }));
        await waitFor(() => expect(api.pickForAgents).toHaveBeenCalledWith("/code/shop", { account: "ankit@example.com", project: "acme-dev" }));
    });

    it("leaves a setup in the person's own shell alone", async () => {
        api.agentPicks.mockResolvedValue({
            project: null,
            default: null,
            effective: { pick: { account: null, project: "mine" }, source: "environment" },
        });
        render(<AgentSetup />);
        expect(await screen.findByText("mine, from your shell")).toBeInTheDocument();
        expect(screen.queryByRole("button", { name: /Google Cloud project for agents/ })).not.toBeInTheDocument();
    });
});
