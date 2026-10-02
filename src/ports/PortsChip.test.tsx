import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { portsApi } from "../api/ports";
import { getState, setState } from "../state/store";
import { agentPort, seedProjects, terminalPort } from "../test/ports";
import { copyPortUrl, openPortExternally, openPortOnDesk, revealPortOwner } from "./portActions";
import { PortsChip } from "./PortsChip";

vi.mock("../api/ports", () => ({ portsApi: { listening: vi.fn(), openExternal: vi.fn() } }));
vi.mock("./portActions", () => ({
    copyPortUrl: vi.fn(),
    openPortExternally: vi.fn(),
    openPortOnDesk: vi.fn(),
    revealPortOwner: vi.fn(),
}));

const initial = getState();
const listening = vi.mocked(portsApi.listening);

async function renderChip() {
    const view = render(<PortsChip sessionId="project" />);
    await act(async () => {
        await Promise.resolve();
    });
    return view;
}

beforeEach(() => {
    vi.clearAllMocks();
    setState(initial, true);
});

afterEach(cleanup);

async function openMenu(name: string) {
    fireEvent.click(screen.getByRole("button", { name }));
    return screen.findByRole("menu", { name: "Listening ports" });
}

describe("PortsChip", () => {
    it("shows nothing while the project listens on no port", async () => {
        seedProjects();
        listening.mockResolvedValue([agentPort(4000, "agent-9")]);
        const { container } = await renderChip();
        expect(container.innerHTML).toBe("");
    });

    it("counts the ports and lists each with its process and owner", async () => {
        seedProjects();
        listening.mockResolvedValue([terminalPort(5173, { paneId: "pane-1", project: "/code" }, "vite"), agentPort(3000, "agent-1")]);
        await renderChip();

        const chip = screen.getByRole("button", { name: "2 listening ports" });
        expect(chip.textContent).toBe("2");
        const menu = await openMenu("2 listening ports");
        expect(menu.hasAttribute("data-overlay")).toBe(true);
        expect([...menu.querySelectorAll(".tb-port-addr")].map((node) => node.textContent)).toEqual(["localhost:3000", "localhost:5173"]);
        expect([...menu.querySelectorAll(".tb-port-meta")].map((node) => node.textContent)).toEqual(["next-server · Claude", "vite · npm run dev"]);
    });

    it("opens a port on the desk of the agent that worked last, with the browser, copy and reveal beside it", async () => {
        seedProjects();
        setState({ agentActivity: { "agent-2": { updatedAt: 9 } } } as never);
        listening.mockResolvedValue([terminalPort(5173, { paneId: "pane-1", project: "/code" })]);
        await renderChip();
        await openMenu("1 listening port");

        expect(screen.getByText("opens on Codex's desk")).toBeTruthy();
        fireEvent.click(screen.getByRole("menuitem", { name: "Open localhost:5173 on Codex's desk" }));
        expect(openPortOnDesk).toHaveBeenCalledWith("agent-2", "http://localhost:5173/");
        expect(screen.queryByRole("menu")).toBeNull();

        await openMenu("1 listening port");
        fireEvent.click(screen.getByRole("menuitem", { name: "Open in your browser" }));
        expect(openPortExternally).toHaveBeenCalledWith("http://localhost:5173/");

        await openMenu("1 listening port");
        fireEvent.click(screen.getByRole("menuitem", { name: "Copy URL" }));
        expect(copyPortUrl).toHaveBeenCalledWith("http://localhost:5173/");

        await openMenu("1 listening port");
        fireEvent.click(screen.getByRole("menuitem", { name: "Show npm run dev" }));
        expect(revealPortOwner).toHaveBeenCalledWith({ kind: "pane", sessionId: "project", windowId: "win-shell", paneId: "pane-1" });
    });

    it("opens in the browser instead when no agent is running, and wakes none", async () => {
        seedProjects({ agentLaunch: "dormant" });
        listening.mockResolvedValue([terminalPort(8080, { project: "/code" })]);
        await renderChip();
        await openMenu("1 listening port");

        expect(screen.getByText("no agent running · opens in your browser")).toBeTruthy();
        expect(screen.queryByRole("menuitem", { name: "Open in your browser" })).toBeNull();
        fireEvent.click(screen.getByRole("menuitem", { name: "Open localhost:8080 in your browser" }));
        expect(openPortExternally).toHaveBeenCalledWith("http://localhost:8080/");
        expect(openPortOnDesk).not.toHaveBeenCalled();
    });

    it("closes on Escape", async () => {
        seedProjects();
        listening.mockResolvedValue([terminalPort(8080, { project: "/code" })]);
        await renderChip();
        await openMenu("1 listening port");
        fireEvent.keyDown(window, { key: "Escape" });
        expect(screen.queryByRole("menu")).toBeNull();
    });
});
