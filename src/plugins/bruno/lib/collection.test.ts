import { beforeEach, expect, it, vi } from "vitest";

const { readDir, readFile } = vi.hoisted(() => ({ readDir: vi.fn(), readFile: vi.fn() }));
vi.mock("../../../plugin-api/host", () => ({ files: { readDir, readFile } }));

const { loadCollection } = await import("./collection");

const FANOUT = 200;

function dirOf(path: string, count: number) {
    return Array.from({ length: count }, (_, index) => ({ name: `req-${index}.bru`, path: `${path}/req-${index}.bru`, is_dir: false }));
}

beforeEach(() => {
    readDir.mockReset();
    readFile.mockReset();
});

it("keeps the reads it has in flight bounded while walking a wide collection", async () => {
    let inFlight = 0;
    let peak = 0;
    readDir.mockImplementation(async (path: string) => (path === "/coll" ? dirOf("/coll", FANOUT) : []));
    readFile.mockImplementation(async () => {
        inFlight += 1;
        peak = Math.max(peak, inFlight);
        await Promise.resolve();
        inFlight -= 1;
        return "meta {\n  name: req\n}\n";
    });

    const collection = await loadCollection("/coll");

    expect(collection.tree).toHaveLength(FANOUT);
    expect(readFile).toHaveBeenCalledTimes(FANOUT);
    expect(peak).toBeLessThanOrEqual(16);
});
