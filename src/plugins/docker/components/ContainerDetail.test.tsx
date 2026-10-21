import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ContainerDetail as Detail } from "../api";

const api = vi.hoisted(() => ({ container: vi.fn() }));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), dockerApi: api }));

import { docker } from "../resources";
import { ContainerDetail, summary } from "./ContainerDetail";

const crashed: Detail = {
    id: "aaaaaaaaaaaa",
    name: "shop-db-1",
    image: "postgres:16",
    imageId: "sha256:1234567890abcdef",
    created: "2026-10-18T10:00:00Z",
    command: ["postgres"],
    entrypoint: ["docker-entrypoint.sh"],
    workingDir: "",
    state: {
        status: "exited",
        running: false,
        exitCode: 137,
        error: null,
        oomKilled: true,
        startedAt: "2026-10-18T10:00:01Z",
        finishedAt: "2026-10-18T10:00:05Z",
        health: { status: "unhealthy", failingStreak: 3, lastOutput: ["pg_isready: no response"] },
    },
    restartCount: 2,
    restartPolicy: "unless-stopped",
    env: ["POSTGRES_PASSWORD", "PGDATA"],
    mounts: [
        {
            kind: "volume",
            source: "/var/lib/docker/volumes/shop_data/_data",
            destination: "/var/lib/postgresql/data",
            writable: true,
            volume: "shop_data",
        },
    ],
    networks: [{ name: "shop_default", ip: null, aliases: ["db"] }],
    ports: [],
    labels: { "com.docker.compose.project": "shop" },
    compose: { project: "shop", service: "db", number: 1, workingDir: "/Users/me/shop", configFiles: [] },
};

afterEach(() => {
    cleanup();
    docker.refresh();
});

beforeEach(() => {
    api.container.mockReset().mockResolvedValue(crashed);
});

describe("a container's detail", () => {
    it("says how a container ended, and that memory ran out", () => {
        expect(summary(crashed)).toMatch(/^Exited \(137\) .* — out of memory$/);
        expect(summary({ ...crashed, state: { ...crashed.state, status: "created", running: false } })).toBe("Created, never started");
    });

    it("shows its state, health, mounts and only the names of its environment variables", async () => {
        const onBack = vi.fn();
        render(<ContainerDetail active name="shop-db-1" tab="inspect" onTab={vi.fn()} onBack={onBack} />);
        expect(await screen.findByRole("heading", { name: "shop-db-1" })).toBeInTheDocument();
        const state = screen.getByRole("region", { name: "State" });
        expect(within(state).getByText("137")).toBeInTheDocument();
        expect(within(state).getByText("2 · policy unless-stopped")).toBeInTheDocument();
        expect(within(screen.getByRole("region", { name: "Health" })).getByText("pg_isready: no response")).toBeInTheDocument();
        expect(within(screen.getByRole("region", { name: "Mounts" })).getByText("shop_data")).toBeInTheDocument();
        const env = screen.getByRole("region", { name: "Environment" });
        expect(within(env).getByText("POSTGRES_PASSWORD")).toBeInTheDocument();
        expect(within(env).getByText("Names only. Sikemux never reads their values.")).toBeInTheDocument();
        fireEvent.click(screen.getByRole("button", { name: "← Containers" }));
        expect(onBack).toHaveBeenCalled();
    });

    it("says why it couldn't read a container that went away", async () => {
        api.container.mockRejectedValue({ category: "not-found", message: "docker: No such container: gone" });
        render(<ContainerDetail active name="gone" tab="inspect" onTab={vi.fn()} onBack={vi.fn()} />);
        expect(await screen.findByText("docker: No such container: gone")).toBeInTheDocument();
    });
});
