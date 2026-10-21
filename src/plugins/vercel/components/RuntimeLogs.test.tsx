import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RuntimeLogLine, VercelDeployment } from "../api";

const api = vi.hoisted(() => ({ runtimeLogs: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), vercelApi: api }));

import { keep, RuntimeLogs } from "./RuntimeLogs";

const line = (patch: Partial<RuntimeLogLine>): RuntimeLogLine => ({
    at: "2026-10-19T05:59:27.000Z",
    kind: "request",
    level: "info",
    message: "/ · prerender · HIT",
    method: "GET",
    path: "/",
    status: 200,
    requestId: "r0",
    deploymentId: "dpl_live",
    ...patch,
});
const failed = line({
    level: "error",
    path: "/api/checkout",
    method: "POST",
    status: 500,
    requestId: "r1",
    message: "/api/checkout · streaming_func · MISS",
});
const threw = line({ kind: "log", level: "error", status: null, requestId: "r1", path: "/api/checkout", message: "TypeError: cart is undefined" });
const missing = line({ level: "warning", path: "/favicon.png", status: 404, requestId: "r2", message: "/[slug] · prerender · MISS" });
const served = line({});
const deployment = { id: "dpl_live", project: "storefront", projectId: "prj_store" } as VercelDeployment;

beforeEach(() => api.runtimeLogs.mockReset().mockResolvedValue({ lines: [threw, failed, missing, served], minutes: 15, filtered: 0, note: null }));
afterEach(cleanup);

describe("runtime logs", () => {
    it("keep a function's line with its request when a status is picked", () => {
        const all = { level: "all" as const, status: "all" as const, path: "" };
        expect(keep([threw, failed, missing, served], { ...all, status: "5xx" })).toEqual([threw, failed]);
        expect(keep([threw, failed, missing, served], { ...all, status: "4xx" })).toEqual([missing]);
        expect(keep([threw, failed, missing, served], { ...all, level: "warning" })).toEqual([threw, failed, missing]);
        expect(keep([threw, failed, missing, served], { ...all, path: "typeerror" })).toEqual([threw]);
    });

    it("read the last minutes asked for, and narrow what shows", async () => {
        render(<RuntimeLogs team="team_acme" deployment={deployment} />);
        const log = screen.getByRole("log", { name: "Runtime log" });
        expect(await within(log).findByText("TypeError: cart is undefined")).toBeInTheDocument();
        expect(api.runtimeLogs).toHaveBeenCalledWith("team_acme", "prj_store", "dpl_live", 15);
        fireEvent.click(within(screen.getByRole("group", { name: "Status" })).getByRole("button", { name: "5xx" }));
        expect(within(log).queryByText("/favicon.png")).not.toBeInTheDocument();
        expect(screen.getByRole("status")).toHaveTextContent("2 of 4");
        fireEvent.click(within(screen.getByRole("group", { name: "Since" })).getByRole("button", { name: "30 min" }));
        await waitFor(() => expect(api.runtimeLogs).toHaveBeenLastCalledWith("team_acme", "prj_store", "dpl_live", 30));
    });

    it("say why there's nothing, in Vercel's words", async () => {
        api.runtimeLogs.mockResolvedValue({ lines: [], minutes: 15, filtered: 0, note: "Nothing in the last 15 minutes." });
        render(<RuntimeLogs team="team_acme" deployment={deployment} />);
        expect(await screen.findByText("Nothing in the last 15 minutes.")).toBeInTheDocument();
    });
});
