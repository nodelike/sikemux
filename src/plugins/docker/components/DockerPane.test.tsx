import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { engineRunning, engineStopped, REDIS, SHOP_DB, SHOP_WEB } from "../testing";

const api = vi.hoisted(() => ({
    status: vi.fn(),
    containers: vi.fn(),
    container: vi.fn(),
    stats: vi.fn(),
    stacks: vi.fn(),
    images: vi.fn(),
    volumes: vi.fn(),
}));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), dockerApi: api }));
const openUrl = vi.hoisted(() => vi.fn(() => Promise.resolve()));
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), openUrl }));

import { docker } from "../resources";
import { DockerPane } from "./DockerPane";

let pane = 0;
const renderPane = () => render(<DockerPane paneId={`docker-${++pane}`} active />);

afterEach(() => {
    cleanup();
    docker.refresh();
});

beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    openUrl.mockClear();
    api.status.mockResolvedValue(engineRunning);
    api.containers.mockResolvedValue([REDIS, SHOP_WEB, SHOP_DB]);
    api.stats.mockResolvedValue([]);
    api.stacks.mockResolvedValue([]);
    api.images.mockResolvedValue([]);
    api.volumes.mockResolvedValue([]);
});

describe("the Docker pane", () => {
    it("says plainly when Docker isn't running, and looks again on Try again", async () => {
        api.status.mockResolvedValue(engineStopped);
        renderPane();
        expect(await screen.findByText("Docker isn't running")).toBeInTheDocument();
        api.status.mockResolvedValue(engineRunning);
        fireEvent.click(screen.getByRole("button", { name: "Try again" }));
        expect(await screen.findByRole("table", { name: "Containers" })).toBeInTheDocument();
    });

    it("lists the engine's containers with their Compose project, and counts the running ones", async () => {
        renderPane();
        const table = await screen.findByRole("table", { name: "Containers" });
        expect(within(table).getAllByRole("row")).toHaveLength(4);
        expect(within(table).getByText("shop · web")).toBeInTheDocument();
        expect(within(table).getByRole("img", { name: "Exited (1) 3 minutes ago" })).toHaveClass("danger");
        const nav = screen.getByRole("navigation", { name: "Docker" });
        expect(within(nav).getByRole("button", { name: /Containers/ })).toHaveTextContent("2/3");
        expect(screen.getByText("v29.2.1")).toBeInTheDocument();
        fireEvent.click(within(nav).getByRole("button", { name: /Compose/ }));
        expect(await screen.findByText(/docker compose up show here/)).toBeInTheDocument();
    });

    it("filters by state and by words in the sidebar's Find box", async () => {
        renderPane();
        const table = await screen.findByRole("table", { name: "Containers" });
        const show = screen.getByRole("group", { name: "Show" });
        fireEvent.click(within(show).getByRole("button", { name: /^Stopped/ }));
        expect(within(table).getAllByRole("row")).toHaveLength(2);
        fireEvent.click(within(show).getByRole("button", { name: /^All/ }));
        fireEvent.change(screen.getByRole("textbox", { name: "Find" }), { target: { value: "shop" } });
        expect(within(table).getAllByRole("row")).toHaveLength(3);
        expect(within(show).getByRole("button", { name: /^Running/ })).toHaveTextContent("1");
        fireEvent.change(screen.getByRole("textbox", { name: "Find" }), { target: { value: "nothing like it" } });
        expect(await screen.findByText("No containers match.")).toBeInTheDocument();
    });

    it("opens a container from its row, and goes back to the list", async () => {
        api.container.mockRejectedValue({ category: "not-found", message: "docker: No such container: redis" });
        renderPane();
        fireEvent.click(await screen.findByRole("row", { name: "redis" }));
        expect(await screen.findByText("docker: No such container: redis")).toBeInTheDocument();
        expect(api.container).toHaveBeenCalledWith("redis");
        fireEvent.click(screen.getByRole("button", { name: "← Containers" }));
        expect(await screen.findByRole("table", { name: "Containers" })).toBeInTheDocument();
    });

    it("opens a container's inspect view from its right-click menu", async () => {
        api.container.mockRejectedValue({ category: "not-found", message: "docker: gone" });
        renderPane();
        fireEvent.contextMenu(await screen.findByRole("row", { name: "redis" }));
        fireEvent.click(await screen.findByRole("menuitem", { name: /Inspect/ }));
        expect(await screen.findByText("docker: gone")).toBeInTheDocument();
        expect(api.container).toHaveBeenCalledWith("redis");
    });

    it("opens a published port in the browser", async () => {
        renderPane();
        fireEvent.click(await screen.findByRole("button", { name: "localhost:3000" }));
        await waitFor(() => expect(openUrl).toHaveBeenCalledWith("http://localhost:3000"));
    });
});
