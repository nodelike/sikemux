import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({}) as ReturnType<typeof import("../testing").mockApi>);
vi.mock("../api", async (importOriginal) => {
    const { mockApi } = await import("../testing");
    Object.assign(api, mockApi());
    return { ...(await importOriginal<object>()), gcpApi: api };
});

import { answerEmpty } from "../testing";
import { errorForAgent, ErrorsPage } from "./ErrorsPage";
import { gcp } from "../resources";

const target = { account: null, project: "acme-prod" };
const group = {
    id: "CJ3kq",
    title: "TypeError: Cannot read properties of undefined (reading 'taxRate')",
    status: "OPEN" as const,
    count: 1290,
    users: 41,
    firstSeen: "2024-10-17T10:00:00Z",
    lastSeen: "2024-10-18T11:00:00Z",
    services: ["checkout"],
    timeline: [1, 4, 9, 30],
    issue: null,
    sample: {
        message: "TypeError: Cannot read properties of undefined (reading 'taxRate')\n    at Totals (totals.js:42:18)",
        at: null,
        service: "checkout",
        version: "0.9.1",
        file: "totals.js",
        line: 42,
        function: "Totals",
        request: { method: "POST", url: "/checkout", status: 500 },
        user: null,
    },
};

beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    answerEmpty(api);
    api.errorGroups.mockResolvedValue([group]);
    api.errorStatus.mockResolvedValue({ id: "CJ3kq", status: "RESOLVED" });
});

afterEach(() => {
    cleanup();
    gcp.refresh();
});

describe("Error Reporting", () => {
    it("hands an agent the error with where it was thrown and how to resolve it", () => {
        const text = errorForAgent(group);
        expect(text).toContain("1290 occurrences, 41 users, status open");
        expect(text).toContain("Thrown at totals.js:42 in Totals");
        expect(text).toContain("Request: POST /checkout → 500");
        expect(text).toContain("error_resolve and name CJ3kq");
    });

    it("lists groups for a time range and opens one to its stack", async () => {
        render(<ErrorsPage paneId="er-1" active target={target} />);
        const table = await screen.findByRole("table", { name: "Error groups" });
        expect(within(table).getByText("1290")).toBeInTheDocument();
        fireEvent.click(screen.getByRole("tab", { name: "7 days" }));
        await waitFor(() => expect(api.errorGroups).toHaveBeenLastCalledWith(target, "7d"));
        fireEvent.click(await screen.findByRole("row", { name: /taxRate/ }));
        expect(await screen.findByText(/at Totals \(totals\.js:42:18\)/)).toBeInTheDocument();
        expect(screen.getByText("totals.js:42 (Totals)")).toBeInTheDocument();
    });

    it("marks a group resolved", async () => {
        render(<ErrorsPage paneId="er-2" active target={target} />);
        fireEvent.click(await screen.findByRole("row", { name: /taxRate/ }));
        fireEvent.click(await screen.findByRole("button", { name: "Resolution status" }));
        fireEvent.click(await screen.findByRole("option", { name: "Resolved" }));
        await waitFor(() => expect(api.errorStatus).toHaveBeenCalledWith(target, "CJ3kq", "RESOLVED"));
    });
});
