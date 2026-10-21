import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Stack } from "./api";

const api = vi.hoisted(() => ({ actOnStack: vi.fn() }));
vi.mock("./api", async (importOriginal) => ({ ...(await importOriginal<object>()), dockerApi: api }));
const host = vi.hoisted(() => ({
    confirmDialog: vi.fn(),
    notify: vi.fn(),
    runInTerminal: vi.fn(() => Promise.resolve()),
}));
vi.mock("../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), ...host }));

import { actOnStack, composeArgv, runCompose } from "./compose";

const shop: Stack = {
    project: "shop",
    workingDir: "/Users/me/shop",
    configFiles: ["/Users/me/shop/compose.yaml", "/Users/me/shop/compose.dev.yaml"],
    services: [],
    running: 2,
    total: 3,
};

beforeEach(() => {
    api.actOnStack.mockReset();
    Object.values(host).forEach((mock) => mock.mockClear());
    host.confirmDialog.mockResolvedValue(true);
});

describe("a Compose stack", () => {
    it("runs compose against the folder and every file it was brought up with", () => {
        expect(composeArgv(shop, "up", "-d")).toEqual([
            "docker",
            "compose",
            "-p",
            "shop",
            "--project-directory",
            "/Users/me/shop",
            "-f",
            "/Users/me/shop/compose.yaml",
            "-f",
            "/Users/me/shop/compose.dev.yaml",
            "up",
            "-d",
        ]);
        expect(composeArgv({ project: "loose", workingDir: null, configFiles: [] }, "logs")).toEqual(["docker", "compose", "-p", "loose", "logs"]);
        runCompose(shop, "Rebuild", "up", "-d", "--build");
        expect(host.runInTerminal).toHaveBeenCalledWith("Rebuild shop", expect.arrayContaining(["--build"]));
    });

    it("asks before taking a stack down, and names a container that refused", async () => {
        host.confirmDialog.mockResolvedValueOnce(false);
        await actOnStack(shop, "down");
        expect(api.actOnStack).not.toHaveBeenCalled();
        api.actOnStack.mockResolvedValueOnce({
            project: "shop",
            done: ["shop-web-1"],
            failed: [{ name: "shop-db-1", message: "docker: permission denied" }],
        });
        await actOnStack(shop, "stop");
        expect(host.notify).toHaveBeenLastCalledWith("error", "shop: 1 refused. shop-db-1: docker: permission denied");
        api.actOnStack.mockResolvedValueOnce({ project: "shop", done: ["shop-web-1"], failed: [] });
        await actOnStack(shop, "restart");
        expect(host.notify).toHaveBeenLastCalledWith("success", "Restarted shop");
    });
});
