import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { portsApi } from "../api/ports";
import { getState, setState } from "../state/store";
import { taskProcessChanged } from "../tasks/processSignal";
import { seedProjects, terminalPort } from "../test/ports";
import { PORT_REFRESH_MS, PORT_SETTLE_MS, useProjectPorts } from "./useProjectPorts";

vi.mock("../api/ports", () => ({ portsApi: { listening: vi.fn(), openExternal: vi.fn() } }));

const initial = getState();
const listening = vi.mocked(portsApi.listening);
let hidden = false;

async function flush() {
    await act(async () => {
        await Promise.resolve();
    });
}

beforeEach(() => {
    vi.useFakeTimers();
    hidden = false;
    vi.spyOn(document, "hidden", "get").mockImplementation(() => hidden);
    listening.mockReset();
    listening.mockResolvedValue([terminalPort(5173, { paneId: "pane-1", project: "/code" })]);
    setState(initial, true);
});

afterEach(() => {
    cleanup();
    vi.useRealTimers();
    vi.restoreAllMocks();
});

describe("useProjectPorts", () => {
    it("reads at once, then every few seconds", async () => {
        seedProjects();
        const { result } = renderHook(() => useProjectPorts("project"));
        await flush();
        expect(listening).toHaveBeenCalledTimes(1);
        expect(result.current.map((port) => port.port)).toEqual([5173]);

        await act(async () => {
            vi.advanceTimersByTime(PORT_REFRESH_MS);
        });
        expect(listening).toHaveBeenCalledTimes(2);
    });

    it("reads nothing while the project has nothing running", async () => {
        seedProjects({ terminal: false, agentLaunch: "dormant" });
        const { result } = renderHook(() => useProjectPorts("project"));
        await act(async () => {
            vi.advanceTimersByTime(PORT_REFRESH_MS * 3);
        });
        expect(listening).not.toHaveBeenCalled();
        expect(result.current).toEqual([]);
    });

    it("stops reading while the window is hidden and reads again when it is shown", async () => {
        seedProjects();
        renderHook(() => useProjectPorts("project"));
        await flush();
        expect(listening).toHaveBeenCalledTimes(1);

        hidden = true;
        act(() => {
            document.dispatchEvent(new Event("visibilitychange"));
        });
        await act(async () => {
            vi.advanceTimersByTime(PORT_REFRESH_MS * 3);
        });
        expect(listening).toHaveBeenCalledTimes(1);

        hidden = false;
        act(() => {
            document.dispatchEvent(new Event("visibilitychange"));
        });
        await flush();
        expect(listening).toHaveBeenCalledTimes(2);
    });

    it("reads when a task starts or stops, and once more after the server has had a moment to bind", async () => {
        seedProjects();
        renderHook(() => useProjectPorts("project"));
        await flush();
        listening.mockClear();

        act(() => taskProcessChanged());
        await flush();
        expect(listening).toHaveBeenCalledTimes(1);

        await act(async () => {
            vi.advanceTimersByTime(PORT_SETTLE_MS);
        });
        expect(listening).toHaveBeenCalledTimes(2);
    });

    it("forgets the ports once the last terminal and agent are gone", async () => {
        seedProjects();
        const { result } = renderHook(() => useProjectPorts("project"));
        await flush();
        expect(result.current).toHaveLength(1);

        act(() => seedProjects({ terminal: false, agentLaunch: "dormant" }));
        expect(result.current).toEqual([]);
        await act(async () => {
            vi.advanceTimersByTime(PORT_REFRESH_MS);
        });
        expect(listening).toHaveBeenCalledTimes(1);
    });
});
