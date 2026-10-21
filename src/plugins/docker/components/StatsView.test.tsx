import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ContainerDetail } from "../api";

const api = vi.hoisted(() => ({ stats: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), dockerApi: api }));

import { docker } from "../resources";
import { Sparkline, StatsView } from "./StatsView";

const web = { id: "bbbbbbbbbbbb", name: "shop-web-1", state: { running: true } } as ContainerDetail;

afterEach(() => {
    cleanup();
    docker.refresh();
});
beforeEach(() => {
    api.stats.mockReset().mockResolvedValue([
        {
            id: "bbbbbbbbbbbb",
            name: "shop-web-1",
            cpuPercent: 12.44,
            memory: 300 * 1024 * 1024,
            memoryLimit: 1200 * 1024 * 1024,
            netRx: 0,
            netTx: 0,
            diskRead: 0,
            diskWrite: 0,
            pids: 24,
        },
    ]);
});

describe("a container's load", () => {
    it("shows its CPU, its memory against its limit and its processes", async () => {
        render(<StatsView active container={web} />);
        const cpu = await screen.findByRole("region", { name: "CPU" });
        expect(within(cpu).getByText("12.4%")).toBeInTheDocument();
        expect(screen.getByRole("meter", { name: "Memory used" })).toHaveAttribute("aria-valuenow", "25");
        expect(screen.getByText("24")).toBeInTheDocument();
    });

    it("measures nothing for a stopped container", () => {
        render(<StatsView active container={{ ...web, state: { ...web.state, running: false } }} />);
        expect(screen.getByText(/It isn't running/)).toBeInTheDocument();
        expect(api.stats).not.toHaveBeenCalled();
    });

    it("draws a line only once there are two samples", () => {
        const { container, rerender } = render(<Sparkline points={[3]} />);
        expect(container.querySelector("polyline")).toBeNull();
        rerender(<Sparkline points={[0, 5, 10]} width={100} height={10} />);
        expect(container.querySelector("polyline")?.getAttribute("points")).toBe("0.0,9.0 50.0,5.0 100.0,1.0");
    });
});
