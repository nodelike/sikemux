import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({}) as ReturnType<typeof import("../testing").mockApi>);
vi.mock("../api", async (importOriginal) => {
    const { mockApi } = await import("../testing");
    Object.assign(api, mockApi());
    return { ...(await importOriginal<object>()), gcpApi: api };
});

import { gcpSettings } from "../state";
import { Problem, disabledApi, plainError } from "./Problem";

const turnedOff =
    "sikemux.gcp: gcp: the compute.googleapis.com API is turned off in this project; turn it on at https://console.developers.google.com/apis/api/compute.googleapis.com/overview?project=acme";

describe("Problem", () => {
    beforeEach(() => gcpSettings.update((settings) => ({ ...settings, account: null, project: "acme" })));
    afterEach(() => {
        cleanup();
        vi.clearAllMocks();
    });

    it("reads a failure without its prefixes and finds the API it names", () => {
        expect(plainError("sikemux.gcp: gcp: too many requests")).toBe("Too many requests");
        expect(disabledApi(turnedOff)).toBe("compute.googleapis.com");
        expect(disabledApi("sikemux.gcp: gcp: http 500: boom")).toBeNull();
    });

    it("offers to enable an API that is turned off, instead of showing the raw failure", async () => {
        api.enableService.mockResolvedValue({ service: "compute.googleapis.com", enabled: true });
        render(<Problem error={turnedOff} what="VM instances" onRetry={vi.fn()} />);
        expect(screen.getByRole("heading", { name: "Compute Engine API" })).toBeTruthy();
        expect(screen.queryByText(/console\.developers/)).toBeNull();
        fireEvent.click(screen.getByRole("button", { name: "Enable" }));
        await waitFor(() => expect(api.enableService).toHaveBeenCalledWith({ account: null, project: "acme" }, "compute.googleapis.com"));
    });

    it("asks for a billing account when Google won't turn the API on without one", async () => {
        api.enableService.mockRejectedValue({ category: "billing-disabled", message: "billing isn't enabled" });
        render(<Problem error={turnedOff} what="VM instances" onRetry={vi.fn()} />);
        fireEvent.click(screen.getByRole("button", { name: "Enable" }));
        expect(await screen.findByRole("button", { name: "Link a billing account" })).toBeTruthy();
    });

    it("lets any other failure be tried again", () => {
        const retry = vi.fn();
        render(<Problem error="sikemux.gcp: gcp: http 500: boom" what="buckets" onRetry={retry} />);
        expect(screen.getByRole("heading", { name: "Couldn't load buckets" })).toBeTruthy();
        expect(screen.getByText("Http 500: boom")).toBeTruthy();
        fireEvent.click(screen.getByRole("button", { name: "Try again" }));
        expect(retry).toHaveBeenCalled();
    });
});
