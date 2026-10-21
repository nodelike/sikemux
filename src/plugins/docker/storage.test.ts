import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Image } from "./api";

const api = vi.hoisted(() => ({ removeImage: vi.fn(), pruneImages: vi.fn(), removeVolume: vi.fn() }));
vi.mock("./api", async (importOriginal) => ({ ...(await importOriginal<object>()), dockerApi: api }));
const host = vi.hoisted(() => ({
    confirmDialog: vi.fn(),
    notify: vi.fn(),
    runInTerminal: vi.fn(() => Promise.resolve()),
}));
vi.mock("../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), ...host }));

import { imageName, pruneImages, removeImage, removeVolume, runImage } from "./storage";

const postgres: Image = { id: "2f9c0b6e8a13", tags: ["postgres:16"], created: 0, size: 1000, containers: 1, dangling: false };

beforeEach(() => {
    Object.values(api).forEach((mock) => mock.mockReset());
    Object.values(host).forEach((mock) => mock.mockClear());
    host.confirmDialog.mockResolvedValue(true);
});

describe("images", () => {
    it("are named by their first tag, or their id when untagged", () => {
        expect(imageName(postgres)).toBe("postgres:16");
        expect(imageName({ ...postgres, tags: [] })).toBe("2f9c0b6e8a13");
    });

    it("are removed with the stopped containers made from them, after saying so", async () => {
        api.removeImage.mockResolvedValueOnce({});
        await removeImage(postgres);
        expect(host.confirmDialog).toHaveBeenCalledWith(expect.objectContaining({ body: "1 stopped container was made from it; they go with it." }));
        expect(api.removeImage).toHaveBeenCalledWith("postgres:16", true);
    });

    it("run in a terminal as a throwaway container", () => {
        runImage(postgres);
        expect(host.runInTerminal).toHaveBeenCalledWith("Run postgres:16", ["docker", "run", "--rm", "-it", "postgres:16"]);
    });

    it("prune only after asking, and say what was freed", async () => {
        api.pruneImages.mockResolvedValueOnce({ deleted: 2, spaceReclaimed: 2048 });
        await pruneImages();
        expect(host.notify).toHaveBeenLastCalledWith("success", "Removed 2 images, freeing 2.0 KB");
    });
});

describe("volumes", () => {
    it("say their data goes with them, and pass on why the engine refused", async () => {
        api.removeVolume.mockRejectedValueOnce({
            category: "conflict",
            message: "docker: volume is in use. Remove the containers that use it first",
        });
        await removeVolume({ name: "shop_pgdata", driver: "local", created: null, size: null, usedBy: [], anonymous: false, composeProject: null });
        expect(host.confirmDialog).toHaveBeenCalledWith(expect.objectContaining({ confirmLabel: "Delete its data", destructive: true }));
        expect(host.notify).toHaveBeenLastCalledWith("error", "docker: volume is in use. Remove the containers that use it first");
    });
});
