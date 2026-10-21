import { beforeEach, describe, expect, it, vi } from "vitest";
import { filesApi, MAX_FRONTEND_FILE_SNAPSHOTS, type ProjectFilesSnapshot } from "./files";

const { invokeCommand } = vi.hoisted(() => ({ invokeCommand: vi.fn() }));

vi.mock("./invoke", () => ({ invokeCommand }));

function deferred<T>() {
    let resolve!: (value: T) => void;
    const promise = new Promise<T>((resolvePromise) => {
        resolve = resolvePromise;
    });
    return { promise, resolve };
}

beforeEach(() => {
    invokeCommand.mockReset();
    filesApi.invalidate();
});

describe("filesApi snapshots", () => {
    it("evicts least-recently-used frontend snapshots", async () => {
        invokeCommand.mockImplementation(async (_command: string, args: { repo: string }) => ({
            scanId: Number(args.repo.slice(1)) + 1,
            files: [`${args.repo}.ts`],
        }));
        for (let index = 0; index < MAX_FRONTEND_FILE_SNAPSHOTS; index++) await filesApi.snapshot(`/${index}`);
        await filesApi.snapshot("/0");
        await filesApi.snapshot(`/${MAX_FRONTEND_FILE_SNAPSHOTS}`);
        expect(filesApi.stats().cacheEntries).toBe(MAX_FRONTEND_FILE_SNAPSHOTS);

        invokeCommand.mockClear();
        await filesApi.snapshot("/0");
        expect(invokeCommand).not.toHaveBeenCalled();
        await filesApi.snapshot("/1");
        expect(invokeCommand).toHaveBeenCalledTimes(1);
        filesApi.evict("/0");
        expect(filesApi.stats().cacheEntries).toBe(MAX_FRONTEND_FILE_SNAPSHOTS - 1);
    });

    it("deduplicates requests and preserves file identity for an unchanged scan", async () => {
        const firstFiles = ["a.ts"];
        invokeCommand.mockResolvedValueOnce({ scanId: 1, files: firstFiles });

        const [first, coalesced] = await Promise.all([filesApi.snapshot("/repo"), filesApi.snapshot("/repo")]);
        expect(coalesced).toBe(first);
        expect(invokeCommand).toHaveBeenCalledTimes(1);
        expect(invokeCommand).toHaveBeenLastCalledWith("list_project_files_snapshot", { repo: "/repo" });

        filesApi.invalidate("/repo");
        invokeCommand.mockResolvedValueOnce({ scanId: 1, files: ["unused.ts"] });
        const unchanged = await filesApi.snapshot("/repo");
        expect(unchanged.files).toBe(firstFiles);

        filesApi.invalidate("/repo");
        invokeCommand.mockResolvedValueOnce({ scanId: 2, files: ["a.ts", "b.ts"] });
        await expect(filesApi.list("/repo")).resolves.toEqual(["a.ts", "b.ts"]);
    });

    it("does not let an invalidated late response replace a newer snapshot", async () => {
        const oldRequest = deferred<ProjectFilesSnapshot>();
        const newRequest = deferred<ProjectFilesSnapshot>();
        invokeCommand.mockReturnValueOnce(oldRequest.promise).mockReturnValueOnce(newRequest.promise);

        const oldResult = filesApi.snapshot("/race");
        filesApi.invalidate("/race");
        const newResult = filesApi.snapshot("/race");
        newRequest.resolve({ scanId: 4, files: ["new.ts"] });
        await expect(newResult).resolves.toEqual({ scanId: 4, files: ["new.ts"] });

        oldRequest.resolve({ scanId: 3, files: ["old.ts"] });
        await expect(oldResult).resolves.toEqual({ scanId: 4, files: ["new.ts"] });
        await expect(filesApi.list("/race")).resolves.toEqual(["new.ts"]);
        expect(invokeCommand).toHaveBeenCalledTimes(2);
    });
});
