import { renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { browserApi } from "../api/browser";
import * as cmd from "./commands";
import { useBrowserReveal } from "./browserReveal";

vi.mock("../api/browser", async () => {
    const actual = await vi.importActual<typeof import("../api/browser")>("../api/browser");
    return { ...actual, browserApi: { ...actual.browserApi, subscribeActing: vi.fn() } };
});

afterEach(() => {
    vi.restoreAllMocks();
    vi.clearAllMocks();
});

describe("revealing the browser", () => {
    it("shows the browser of each agent that starts acting in it, and stops listening once unmounted", async () => {
        const reveal = vi.spyOn(cmd, "revealBrowserPane").mockImplementation(() => {});
        let deliver: (agentId: string) => void = () => {};
        let aborted = false;
        vi.mocked(browserApi.subscribeActing).mockImplementation(async (listener, signal) => {
            deliver = listener;
            signal.addEventListener("abort", () => (aborted = true));
            return () => {};
        });

        const { unmount } = renderHook(() => useBrowserReveal());
        await vi.waitFor(() => expect(browserApi.subscribeActing).toHaveBeenCalled());
        deliver("agent-one");

        expect(reveal).toHaveBeenCalledWith("agent-one");
        unmount();
        expect(aborted).toBe(true);
    });
});
