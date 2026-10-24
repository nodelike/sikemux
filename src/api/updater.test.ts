import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getState, setState, type PendingUpdate } from "../state/store";
import { useToasts } from "../state/toast";
import type { UpdateStatus } from "./generated/wire";
import { MemoryIpcTransport, installIpcTransportForTests, resetIpcTransportForTests } from "./transport";
import { checkForUpdate, checkForUpdateNow, installPendingUpdate, updateDownloadPercent } from "./updater";

const initial = getState();
let transport: MemoryIpcTransport;
let core: Record<string, (params: unknown) => Promise<unknown>>;
let restarts: ReturnType<typeof vi.fn<() => Promise<void>>>;

function status(overrides: Partial<UpdateStatus> = {}): UpdateStatus {
    return {
        updates: true,
        current: "0.2.0",
        checked: { at: 1, channel: "stable", error: null },
        available: null,
        state: "idle",
        error: null,
        blockers: [],
        downloadedBytes: 0,
        totalBytes: null,
        forced: false,
        ...overrides,
    };
}

const release = { version: "0.2.1", notes: "Fix updater progress", date: "2026-08-14T00:00:00Z", credits: null };

function availableUpdate(overrides: Partial<PendingUpdate> = {}): PendingUpdate {
    return { ...release, currentVersion: "0.2.0", state: "available", error: null, downloadedBytes: 0, totalBytes: null, ...overrides };
}

beforeEach(() => {
    resetIpcTransportForTests();
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
    core = { updateStatus: () => Promise.resolve(status()) };
    transport.register("core_call", (args) => {
        const { method, params } = args as { method: string; params: unknown };
        const answer = core[method];
        return answer ? answer(params) : Promise.reject(new Error(`no ${method}`));
    });
    restarts = vi.fn<() => Promise<void>>(() => Promise.resolve());
    transport.register("update_restart", restarts);
    setState(initial, true);
    useToasts.setState({ toasts: [] });
});

afterEach(() => {
    resetIpcTransportForTests();
    setState(initial, true);
});

describe("updater progress", () => {
    it("clamps percentages", () => {
        expect(updateDownloadPercent(availableUpdate({ downloadedBytes: 51, totalBytes: 100 }))).toBe(51);
        expect(updateDownloadPercent(availableUpdate({ downloadedBytes: 110, totalBytes: 100 }))).toBe(100);
        expect(updateDownloadPercent(availableUpdate())).toBeNull();
    });

    it("shows the core's download and restarts once the update is ready", async () => {
        setState({ pendingUpdate: availableUpdate() });
        core.updateInstall = () => Promise.resolve(status({ available: release, state: "downloading", downloadedBytes: 25, totalBytes: 100 }));

        await installPendingUpdate();
        expect(getState().pendingUpdate).toMatchObject({ state: "downloading", downloadedBytes: 25, totalBytes: 100 });
        expect(restarts).not.toHaveBeenCalled();

        core.updateCheck = () => Promise.resolve(status({ available: release, state: "ready", downloadedBytes: 100, totalBytes: 100 }));
        await checkForUpdate();

        expect(restarts).toHaveBeenCalledOnce();
        expect(getState().pendingUpdate).toMatchObject({ state: "restarting", downloadedBytes: 100 });
        expect(getState().lastReleaseNotes).toEqual({ version: "0.2.1", notes: "Fix updater progress", date: "2026-08-14T00:00:00Z", credits: null });
    });

    it("deduplicates concurrent installs and exposes the core's failure for retry", async () => {
        setState({ pendingUpdate: availableUpdate() });
        const install = vi.fn(() => Promise.reject(new Error("download timed out")));
        core.updateInstall = install;

        const first = installPendingUpdate();
        const second = installPendingUpdate();
        expect(second).toBe(first);
        await first;

        expect(install).toHaveBeenCalledOnce();
        expect(restarts).not.toHaveBeenCalled();
        expect(getState().pendingUpdate).toMatchObject({ state: "error", error: "download timed out" });
    });
});

describe("update checks", () => {
    it("shows a failed background check without a toast", async () => {
        core.updateCheck = () => Promise.reject(new Error("update check: network unreachable"));
        core.updateStatus = () => Promise.resolve(status({ checked: { at: 1, channel: "stable", error: "network unreachable" } }));

        await checkForUpdate();

        expect(getState().pendingUpdate).toBeNull();
        expect(getState().lastUpdateCheck).toMatchObject({ channel: "stable", error: "network unreachable" });
        expect(useToasts.getState().toasts).toEqual([]);
    });

    it("reports a user-initiated failure so a broken feed is not mistaken for being current", async () => {
        core.updateCheck = () => Promise.reject(new Error("update check: 404 Not Found"));

        await checkForUpdateNow();

        expect(useToasts.getState().toasts).toMatchObject([{ kind: "error", text: "update check: update check: 404 Not Found" }]);
    });

    it("tells a person who checks by hand that they are offline, not that something broke", async () => {
        core.updateCheck = () => Promise.reject({ category: "offline", message: "Can't reach the server. Check your connection." });

        await checkForUpdateNow();

        expect(useToasts.getState().toasts).toMatchObject([
            { kind: "info", text: "You're offline. Sikemux will check for updates when you're back." },
        ]);
    });

    it("confirms an up-to-date result only when the check actually succeeded", async () => {
        core.updateCheck = () => Promise.resolve(status());

        await checkForUpdateNow();

        expect(getState().pendingUpdate).toBeNull();
        expect(getState().lastUpdateCheck).toMatchObject({ error: null });
        expect(useToasts.getState().toasts).toMatchObject([{ kind: "info", text: "Sikemux is up to date on the stable channel." }]);
    });

    it("announces an available update and deduplicates concurrent manual checks", async () => {
        const check = vi.fn(() =>
            Promise.resolve(status({ current: "0.2.3", available: { version: "0.3.0", notes: null, date: null, credits: null } })),
        );
        core.updateCheck = check;

        const first = checkForUpdateNow();
        expect(checkForUpdateNow()).toBe(first);
        await first;

        expect(check).toHaveBeenCalledOnce();
        expect(getState().pendingUpdate).toMatchObject({ version: "0.3.0", state: "available" });
        expect(useToasts.getState().toasts).toMatchObject([{ kind: "success", text: "Update v0.3.0 is ready to install." }]);
    });
});
