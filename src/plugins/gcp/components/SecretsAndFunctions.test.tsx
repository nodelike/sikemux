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
import { FunctionsPage } from "./FunctionsPage";
import { period, SecretsPage } from "./SecretsPage";
import { gcp } from "../resources";

const target = { account: null, project: "acme-prod" };

beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    answerEmpty(api);
    api.secrets.mockResolvedValue([
        { name: "db-password", replication: "automatic", createdAt: null, expiresAt: null, nextRotation: null, rotationPeriod: "2592000s" },
    ]);
    api.secret.mockResolvedValue({
        name: "db-password",
        replication: "automatic",
        createdAt: null,
        expiresAt: null,
        nextRotation: null,
        rotationPeriod: "2592000s",
        versions: [
            { version: "3", state: "ENABLED", createdAt: null, destroyedAt: null },
            { version: "2", state: "DISABLED", createdAt: null, destroyedAt: null },
        ],
    });
    api.secretVersion.mockResolvedValue({});
    api.functions.mockResolvedValue([
        {
            name: "resize-image",
            location: "us-central1",
            state: "ACTIVE",
            generation: "2nd gen",
            runtime: "nodejs20",
            entryPoint: "resize",
            url: "https://resize-image-abc.a.run.app",
            memory: "512M",
            timeoutSeconds: 60,
            maxInstances: 10,
            serviceAccount: null,
            runService: "resize-image",
            trigger: { kind: "event", event: "google.cloud.pubsub.topic.v1.messagePublished", topic: "uploads", retry: true },
            env: ["BUCKET"],
            secretEnv: ["API_KEY"],
            updatedAt: null,
            message: null,
        },
    ]);
    confirm.mockClear();
});

afterEach(() => {
    cleanup();
    gcp.refresh();
});

describe("Secret Manager", () => {
    it("reads rotation periods in days", () => {
        expect(period("2592000s")).toBe("Every 30 days");
        expect(period("86400s")).toBe("Every 1 day");
        expect(period(null)).toBeNull();
    });

    it("disables a version once the person confirms, and never shows a value", async () => {
        render(<SecretsPage paneId="sm-1" active target={target} />);
        fireEvent.click(await screen.findByRole("row", { name: /db-password/ }));
        const versions = await screen.findByRole("table", { name: "Versions" });
        fireEvent.click(within(versions).getByRole("button", { name: "Disable" }));
        await waitFor(() => expect(api.secretVersion).toHaveBeenCalledWith(target, "db-password", "3", false));
        expect(confirm).toHaveBeenCalledWith(expect.objectContaining({ destructive: true }));
        expect(screen.getByText(/never read here/)).toBeInTheDocument();
    });
});

describe("Cloud Functions", () => {
    it("lists functions by trigger and opens one to its variables by name", async () => {
        render(<FunctionsPage paneId="fn-1" active target={target} />);
        expect(await screen.findByText("Pub/Sub: uploads")).toBeInTheDocument();
        fireEvent.click(screen.getByRole("row", { name: /resize-image/ }));
        fireEvent.click(await screen.findByRole("tab", { name: "Details" }));
        expect(screen.getByText("API_KEY")).toBeInTheDocument();
        expect(screen.getByText("Secret Manager")).toBeInTheDocument();
    });
});
