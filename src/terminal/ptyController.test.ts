import { describe, expect, it, vi } from "vitest";
import {
    PtyControllerDisposedError,
    PtyControllerExitedError,
    PtyControllerReplacedError,
    PtyLifecycleController,
    PtySubscriptionOverflowError,
    parsePtyShellMetadataSnapshot,
    type PtyApi,
    type PtyAttachResult,
    type PtyChannelAdapter,
    type PtyControllerErrorEvent,
    type PtyOutputChunk,
    type PtySpawnRequest,
    type PtyTimerAdapter,
} from "./ptyController";

type TestContext = { readonly paneId: string };
type FakeChannel = { readonly id: number };

function deferred<T>() {
    let resolve!: (value: T | PromiseLike<T>) => void;
    let reject!: (reason?: unknown) => void;
    const promise = new Promise<T>((resolvePromise, rejectPromise) => {
        resolve = resolvePromise;
        reject = rejectPromise;
    });
    return { promise, resolve, reject };
}

class FakeChannels implements PtyChannelAdapter<FakeChannel> {
    readonly bindings: {
        transport: FakeChannel;
        emit: (chunk: PtyOutputChunk) => void;
        close: ReturnType<typeof vi.fn>;
    }[] = [];

    create(onMessage: (chunk: PtyOutputChunk) => void) {
        const binding = {
            transport: { id: this.bindings.length + 1 },
            emit: onMessage,
            close: vi.fn(),
        };
        this.bindings.push(binding);
        return { transport: binding.transport, close: binding.close };
    }
}

class FakeTimer implements PtyTimerAdapter {
    readonly jobs = new Map<number, () => void>();
    readonly schedule = vi.fn((callback: () => void, _delayMs: number): number => {
        const id = this.jobs.size + 1;
        this.jobs.set(id, callback);
        return id;
    });
    readonly cancel = vi.fn((handle: unknown) => {
        this.jobs.delete(handle as number);
    });

    runAll(): void {
        const callbacks = Array.from(this.jobs.values());
        this.jobs.clear();
        callbacks.forEach((callback) => callback());
    }
}

function fakeApi() {
    const spawn = vi.fn(async (_request: PtySpawnRequest<TestContext>) => 42);
    const write = vi.fn(async (_id: number, _data: string) => {});
    const resize = vi.fn(async (_id: number, _cols: number, _rows: number) => {});
    const kill = vi.fn(async (_id: number) => {});
    const attach = vi.fn(async (_id: number, _channel: FakeChannel): Promise<PtyAttachResult> => {
        return { subId: 7, snapshot: new Uint8Array([1, 2]), alternateScreen: false };
    });
    const detach = vi.fn(async (_id: number, _subId: number) => {});
    const ack = vi.fn(async (_id: number, _subId: number, _bytes: number) => {});
    const api = { spawn, write, resize, kill, attach, detach, ack } satisfies PtyApi<FakeChannel, TestContext>;
    return { api, spawn, write, resize, kill, attach, detach, ack };
}

function controllerOptions(overrides: Partial<ConstructorParameters<typeof PtyLifecycleController<FakeChannel, TestContext>>[0]> = {}) {
    const fakes = fakeApi();
    const channels = new FakeChannels();
    const errors: PtyControllerErrorEvent[] = [];
    return {
        fakes,
        channels,
        errors,
        options: {
            api: fakes.api,
            channels,
            cwd: "/repo",
            startup: "codex",
            context: { paneId: "pane-1" },
            onError: (event: PtyControllerErrorEvent) => errors.push(event),
            ...overrides,
        },
    };
}

describe("PtyLifecycleController process ownership", () => {
    it("coalesces concurrent starts and owns write and resize operations", async () => {
        const spawned = deferred<number>();
        const { fakes, options } = controllerOptions();
        fakes.spawn.mockReturnValue(spawned.promise);
        const controller = new PtyLifecycleController(options);

        const first = controller.start();
        const second = controller.start();
        expect(first).toBe(second);
        expect(fakes.spawn).toHaveBeenCalledOnce();
        expect(fakes.spawn).toHaveBeenCalledWith({
            cols: 80,
            rows: 24,
            cwd: "/repo",
            startup: "codex",
            directCommand: null,
            context: { paneId: "pane-1" },
        });

        spawned.resolve(42);
        await expect(first).resolves.toBe(42);
        await controller.write("echo hi\r");
        await controller.resize(120, 40);

        expect(fakes.write).toHaveBeenCalledTimes(1);
        expect(fakes.write).toHaveBeenCalledWith(42, "echo hi\r");
        expect(fakes.resize).toHaveBeenCalledTimes(1);
        expect(fakes.resize).toHaveBeenCalledWith(42, 120, 40);
        expect(controller.getSnapshot()).toMatchObject({
            status: "running",
            spawnAttempts: 1,
            cols: 120,
            rows: 40,
            processOwnership: "controller",
        });
    });

    it("adopts an existing PTY without spawning or taking kill ownership", async () => {
        const { fakes, options } = controllerOptions({ existingPtyId: 73 });
        const controller = new PtyLifecycleController(options);

        const first = controller.start();
        const second = controller.start();
        expect(first).toBe(second);
        await expect(first).resolves.toBe(73);
        await controller.write("input");
        await controller.resize(132, 43);
        const attachment = await controller.attach(vi.fn());
        await attachment.detach();
        await controller.dispose();

        expect(fakes.spawn).not.toHaveBeenCalled();
        expect(fakes.write).toHaveBeenCalledWith(73, "input");
        expect(fakes.resize).toHaveBeenCalledWith(73, 132, 43);
        expect(fakes.attach).toHaveBeenCalledWith(73, expect.anything());
        expect(fakes.detach).toHaveBeenCalledWith(73, 7);
        expect(fakes.kill).not.toHaveBeenCalled();
        expect(controller.getSnapshot()).toMatchObject({
            status: "disposed",
            spawnAttempts: 0,
            attachmentCount: 0,
            processOwnership: "external",
        });
    });

    it("detaches a late external attachment when disposal wins without killing", async () => {
        const nativeAttach = deferred<PtyAttachResult>();
        const { fakes, channels, options } = controllerOptions({ existingPtyId: 75 });
        fakes.attach.mockReturnValue(nativeAttach.promise);
        const controller = new PtyLifecycleController(options);
        const attaching = controller.attach(vi.fn());
        await vi.waitFor(() => expect(channels.bindings).toHaveLength(1));

        const disposing = controller.dispose();
        nativeAttach.resolve({ subId: 20, snapshot: new Uint8Array([1]), alternateScreen: false });

        await expect(attaching).rejects.toBeInstanceOf(PtyControllerDisposedError);
        await disposing;
        expect(fakes.detach).toHaveBeenCalledOnce();
        expect(fakes.detach).toHaveBeenCalledWith(75, 20);
        expect(fakes.kill).not.toHaveBeenCalled();
        expect(channels.bindings[0].close).toHaveBeenCalledOnce();
    });

    it("replaces an external PTY and quarantines its stale attachment generation", async () => {
        const oldNativeAttach = deferred<PtyAttachResult>();
        const { fakes, channels, options } = controllerOptions({ existingPtyId: 76 });
        fakes.attach
            .mockReturnValueOnce(oldNativeAttach.promise)
            .mockResolvedValueOnce({ subId: 22, snapshot: new Uint8Array([7, 8]), alternateScreen: false });
        const controller = new PtyLifecycleController(options);
        const oldListener = vi.fn();
        const oldAttaching = controller.attach(oldListener);
        await vi.waitFor(() => expect(fakes.attach).toHaveBeenCalledWith(76, expect.anything()));

        await expect(controller.adoptExistingPty(77)).resolves.toBe(77);
        expect(channels.bindings[0].close).toHaveBeenCalledOnce();
        channels.bindings[0].emit([99]);
        expect(oldListener).not.toHaveBeenCalled();

        const newListener = vi.fn();
        const replacement = await controller.attach(newListener);
        replacement.activate();
        channels.bindings[1].emit([10]);
        expect(newListener).toHaveBeenCalledWith([10]);
        expect(fakes.attach).toHaveBeenNthCalledWith(2, 77, expect.anything());

        oldNativeAttach.resolve({ subId: 21, snapshot: new Uint8Array([1, 2]), alternateScreen: false });
        await expect(oldAttaching).rejects.toBeInstanceOf(PtyControllerReplacedError);
        expect(fakes.detach).toHaveBeenCalledWith(76, 21);

        await controller.resize(140, 50);
        expect(fakes.resize).toHaveBeenCalledWith(77, 140, 50);
        await controller.dispose();
        expect(fakes.detach).toHaveBeenCalledWith(77, 22);
        expect(fakes.kill).not.toHaveBeenCalled();
    });

    it("recovers an exited external controller only through explicit replacement", async () => {
        const missing = Object.freeze({ category: "pty-not-found", message: "gone" });
        const { fakes, options } = controllerOptions({ existingPtyId: 78 });
        fakes.attach.mockRejectedValueOnce(missing).mockResolvedValueOnce({ subId: 23, snapshot: new Uint8Array([3]), alternateScreen: false });
        const controller = new PtyLifecycleController(options);

        await expect(controller.attach(vi.fn())).rejects.toBe(missing);
        await expect(controller.start()).rejects.toBeInstanceOf(PtyControllerExitedError);
        await expect(controller.adoptExistingPty(79)).resolves.toBe(79);
        const recovered = await controller.attach(vi.fn());

        expect(recovered.snapshot).toEqual(new Uint8Array([3]));
        expect(fakes.attach).toHaveBeenNthCalledWith(2, 79, expect.anything());
        expect(fakes.spawn).not.toHaveBeenCalled();
        await controller.dispose();
        expect(fakes.kill).not.toHaveBeenCalled();
    });

    it("serializes native resizes and coalesces queued requests to the newest size", async () => {
        const firstResize = deferred<void>();
        const latestResize = deferred<void>();
        const { fakes, options } = controllerOptions();
        fakes.resize.mockReturnValueOnce(firstResize.promise).mockReturnValueOnce(latestResize.promise);
        const controller = new PtyLifecycleController(options);
        await controller.start();

        const first = controller.resize(100, 30);
        await vi.waitFor(() => expect(fakes.resize).toHaveBeenCalledOnce());
        const superseded = controller.resize(120, 40);
        const latest = controller.resize(140, 50);

        expect(superseded).toBe(latest);
        expect(fakes.resize).toHaveBeenCalledOnce();
        firstResize.resolve();
        await first;
        await vi.waitFor(() => expect(fakes.resize).toHaveBeenCalledTimes(2));
        expect(fakes.resize).toHaveBeenNthCalledWith(2, 42, 140, 50);
        latestResize.resolve();
        await latest;

        expect(controller.getSnapshot()).toMatchObject({ cols: 140, rows: 50 });
        expect(fakes.resize).toHaveBeenNthCalledWith(1, 42, 100, 30);
    });

    it("does not publish a stale resize failure when a newer resize is queued", async () => {
        const firstResize = deferred<void>();
        const latestResize = deferred<void>();
        const staleError = new Error("obsolete resize failed");
        const { fakes, errors, options } = controllerOptions();
        fakes.resize.mockReturnValueOnce(firstResize.promise).mockReturnValueOnce(latestResize.promise);
        const controller = new PtyLifecycleController(options);
        await controller.start();

        const first = controller.resize(100, 30);
        await vi.waitFor(() => expect(fakes.resize).toHaveBeenCalledOnce());
        const latest = controller.resize(140, 50);
        firstResize.reject(staleError);

        await expect(first).rejects.toBe(staleError);
        await vi.waitFor(() => expect(fakes.resize).toHaveBeenCalledTimes(2));
        expect(controller.getSnapshot().failureOperation).toBeNull();
        expect(errors).not.toContainEqual({ operation: "resize", error: staleError });

        latestResize.resolve();
        await latest;
        expect(controller.getSnapshot()).toMatchObject({ cols: 140, rows: 50, failureOperation: null });
    });

    it("publishes the latest resize failure and preserves it for awaiters", async () => {
        const resizeError = new Error("native resize failed");
        const { fakes, errors, options } = controllerOptions();
        fakes.resize.mockRejectedValue(resizeError);
        const controller = new PtyLifecycleController(options);
        await controller.start();

        const resizing = controller.resize(110, 35);
        await vi.waitFor(() => expect(errors).toContainEqual({ operation: "resize", error: resizeError }));

        expect(controller.getSnapshot()).toMatchObject({ cols: 80, rows: 24, failureOperation: "resize" });
        await expect(resizing).rejects.toBe(resizeError);
    });

    it("retries a failed spawn without killing a process that never existed", async () => {
        const spawnError = Object.freeze({ category: "pty", detail: "spawn failed" });
        const { fakes, errors, options } = controllerOptions();
        fakes.spawn.mockRejectedValueOnce(spawnError).mockResolvedValueOnce(77);
        const controller = new PtyLifecycleController(options);

        await expect(controller.start()).rejects.toBe(spawnError);
        expect(controller.getSnapshot()).toMatchObject({ status: "failed", spawnAttempts: 1, failureOperation: "spawn" });
        await expect(controller.start()).resolves.toBe(77);

        expect(fakes.spawn).toHaveBeenCalledTimes(2);
        expect(fakes.kill).not.toHaveBeenCalled();
        expect(errors).toContainEqual({ operation: "spawn", error: spawnError });
    });

    it("treats an invalid native spawn ID as a retryable spawn failure", async () => {
        const { fakes, options } = controllerOptions();
        fakes.spawn.mockResolvedValueOnce(-1).mockResolvedValueOnce(88);
        const controller = new PtyLifecycleController(options);

        await expect(controller.start()).rejects.toThrow("invalid runtime ID");
        expect(controller.getSnapshot()).toMatchObject({ status: "failed", spawnAttempts: 1, failureOperation: "spawn" });
        await expect(controller.start()).resolves.toBe(88);
        expect(fakes.spawn).toHaveBeenCalledTimes(2);
    });

    it("kills exactly once when disposed during a coalesced start", async () => {
        const spawned = deferred<number>();
        const timer = new FakeTimer();
        const { fakes, options } = controllerOptions({ initialInput: "private first task", timer });
        fakes.spawn.mockReturnValue(spawned.promise);
        const controller = new PtyLifecycleController(options);
        const starting = controller.start();
        const disposing = controller.dispose();

        expect(controller.dispose()).toBe(disposing);
        spawned.resolve(91);

        await expect(starting).rejects.toBeInstanceOf(PtyControllerDisposedError);
        await disposing;
        expect(fakes.kill).toHaveBeenCalledTimes(1);
        expect(fakes.kill).toHaveBeenCalledWith(91);
        expect(fakes.write).not.toHaveBeenCalled();
        expect(timer.schedule).not.toHaveBeenCalled();
        expect(controller.getSnapshot()).toMatchObject({ status: "disposed", attachmentCount: 0, initialInput: "cancelled" });
    });

    it("takes back a process that is still there instead of spawning, without sending its first input again", async () => {
        const timer = new FakeTimer();
        const onProcess = vi.fn();
        const resume = vi.fn(async (_id: number) => true);
        const { fakes, options } = controllerOptions({ resumePtyId: 1_759_300_000_005, initialInput: "Build it.", timer, onProcess });
        const controller = new PtyLifecycleController({ ...options, api: { ...fakes.api, resume } });

        await expect(controller.start()).resolves.toBe(1_759_300_000_005);
        timer.runAll();
        expect(resume).toHaveBeenCalledWith(1_759_300_000_005);
        expect(fakes.spawn).not.toHaveBeenCalled();
        expect(fakes.write).not.toHaveBeenCalled();
        expect(onProcess).toHaveBeenCalledWith(1_759_300_000_005);
        expect(controller.getSnapshot()).toMatchObject({ status: "running", processOwnership: "controller", initialInput: "none" });

        await controller.dispose();
        expect(fakes.kill).toHaveBeenCalledWith(1_759_300_000_005);
    });

    it("spawns a fresh process when the saved one is gone or cannot be checked", async () => {
        for (const resume of [vi.fn(async () => false), vi.fn(async () => Promise.reject(new Error("core unreachable")))]) {
            const onProcess = vi.fn();
            const { fakes, options } = controllerOptions({ resumePtyId: 9, onProcess });
            const controller = new PtyLifecycleController({ ...options, api: { ...fakes.api, resume } });
            await expect(controller.start()).resolves.toBe(42);
            expect(fakes.spawn).toHaveBeenCalledOnce();
            expect(onProcess).toHaveBeenCalledWith(42);
        }
    });

    it("never takes over a process for a controller borrowing another owner's", async () => {
        const resume = vi.fn(async () => true);
        const { fakes, options } = controllerOptions({ existingPtyId: 73, resumePtyId: 9 });
        const controller = new PtyLifecycleController({ ...options, api: { ...fakes.api, resume } });
        await expect(controller.start()).resolves.toBe(73);
        expect(resume).not.toHaveBeenCalled();
    });

    it("delivers initial input at most once and reports successful delivery", async () => {
        const timer = new FakeTimer();
        const delivered = vi.fn();
        const { fakes, options } = controllerOptions({ initialInput: "  Build it safely.  ", timer, onInitialInputDelivered: delivered });
        const controller = new PtyLifecycleController(options);

        await controller.start();
        expect(controller.getSnapshot().initialInput).toBe("scheduled");
        expect(fakes.write).not.toHaveBeenCalled();
        timer.runAll();
        await vi.waitFor(() => expect(controller.getSnapshot().initialInput).toBe("delivered"));
        timer.runAll();

        expect(fakes.write).toHaveBeenCalledTimes(1);
        expect(fakes.write).toHaveBeenCalledWith(42, "\x1b[200~Build it safely.\x1b[201~\r");
        expect(delivered).toHaveBeenCalledOnce();
    });

    it("replays dropped paths as distinct paste events before submitting the first task", async () => {
        const timer = new FakeTimer();
        const delivered = vi.fn();
        const { fakes, options } = controllerOptions({
            initialPastes: ["'/tmp/first image.png'", "'/tmp/second image.jpg'"],
            initialInput: "Compare these screenshots.",
            timer,
            onInitialInputDelivered: delivered,
        });
        const controller = new PtyLifecycleController(options);

        await controller.start();
        timer.runAll();
        await vi.waitFor(() => expect(controller.getSnapshot().initialInput).toBe("delivered"));

        expect(fakes.write).toHaveBeenCalledOnce();
        expect(fakes.write).toHaveBeenCalledWith(
            42,
            "\x1b[200~'/tmp/first image.png'\x1b[201~\x1b[200~'/tmp/second image.jpg'\x1b[201~\x1b[200~Compare these screenshots.\x1b[201~\r",
        );
        expect(delivered).toHaveBeenCalledOnce();
    });

    it("does not retry an ambiguous failed initial-input write", async () => {
        const timer = new FakeTimer();
        const writeError = new Error("bridge response lost");
        const { fakes, errors, options } = controllerOptions({ initialInput: "run once", timer });
        fakes.write.mockRejectedValue(writeError);
        const controller = new PtyLifecycleController(options);

        await controller.start();
        timer.runAll();
        await vi.waitFor(() => expect(controller.getSnapshot().initialInput).toBe("failed"));
        await controller.start();
        timer.runAll();

        expect(fakes.write).toHaveBeenCalledTimes(1);
        expect(errors).toContainEqual({ operation: "initial-input", error: writeError });
    });
});

describe("PtyLifecycleController renderer subscriptions", () => {
    it("keeps atomic snapshot ordering across attach, activate, and detach", async () => {
        const attached = deferred<PtyAttachResult>();
        const { fakes, channels, options } = controllerOptions();
        fakes.attach.mockReturnValue(attached.promise);
        const controller = new PtyLifecycleController(options);
        const received: PtyOutputChunk[] = [];
        const attaching = controller.attach((chunk) => received.push(chunk));
        await vi.waitFor(() => expect(channels.bindings).toHaveLength(1));

        const beforeResponse = [3, 4];
        channels.bindings[0].emit(beforeResponse);
        attached.resolve({ subId: 9, snapshot: new Uint8Array([1, 2]), alternateScreen: true });
        const attachment = await attaching;
        const beforeActivate = [5, 6];
        channels.bindings[0].emit(beforeActivate);
        expect(received).toEqual([]);
        expect(attachment.snapshot).toEqual(new Uint8Array([1, 2]));
        expect(attachment.alternateScreen).toBe(true);

        attachment.activate();
        expect(received).toEqual([beforeResponse, beforeActivate]);
        const live = new Uint8Array([7, 8]);
        channels.bindings[0].emit(live);
        expect(received).toEqual([beforeResponse, beforeActivate, live]);

        const detaching = attachment.detach();
        expect(attachment.detach()).toBe(detaching);
        await detaching;
        channels.bindings[0].emit([9]);
        expect(received).toHaveLength(3);
        expect(fakes.detach).toHaveBeenCalledTimes(1);
        expect(fakes.detach).toHaveBeenCalledWith(42, 9);
        expect(fakes.kill).not.toHaveBeenCalled();
        expect(channels.bindings[0].close).toHaveBeenCalledOnce();
        expect(controller.getSnapshot()).toMatchObject({ status: "running", attachmentCount: 0 });
    });

    it("delivers raw IPC bytes as a view over the same buffer", async () => {
        const { fakes, channels, options } = controllerOptions();
        const controller = new PtyLifecycleController(options);
        const received: PtyOutputChunk[] = [];
        const attachment = await controller.attach((chunk) => received.push(chunk));
        attachment.activate();
        expect(fakes.attach).toHaveBeenCalledOnce();

        const buffer = new Uint8Array([27, 91, 75]).buffer;
        channels.bindings[0].emit(buffer as unknown as PtyOutputChunk);
        expect(received).toHaveLength(1);
        const chunk = received[0] as Uint8Array;
        expect(chunk).toBeInstanceOf(Uint8Array);
        expect(chunk.buffer).toBe(buffer);
        expect(Array.from(chunk)).toEqual([27, 91, 75]);
    });

    it("acks written bytes against the subscription and stops once detached", async () => {
        const { fakes, options } = controllerOptions();
        const controller = new PtyLifecycleController(options);
        const attachment = await controller.attach(vi.fn());

        attachment.ack(4_096);
        await vi.waitFor(() => expect(fakes.ack).toHaveBeenCalledWith(42, 7, 4_096));

        attachment.ack(0);
        attachment.ack(-1);
        expect(fakes.ack).toHaveBeenCalledOnce();

        await attachment.detach();
        attachment.ack(4_096);
        expect(fakes.ack).toHaveBeenCalledOnce();
    });

    it("validates and freezes the opt-in shell snapshot returned by attach", async () => {
        const { fakes, options } = controllerOptions();
        fakes.attach.mockResolvedValue({
            subId: 18,
            snapshot: new Uint8Array([]),
            alternateScreen: false,
            shell: { revision: 7, cwd: "/repo", phase: "prompt", lastExitCode: 0 },
        });
        const controller = new PtyLifecycleController(options);

        const attachment = await controller.attach(vi.fn());

        expect(attachment.shell).toEqual({ revision: 7, cwd: "/repo", phase: "prompt", lastExitCode: 0 });
        expect(Object.isFrozen(attachment.shell)).toBe(true);
    });

    it("rejects invalid shell metadata without ending the live process", async () => {
        const { fakes, options } = controllerOptions();
        fakes.attach.mockResolvedValue({
            subId: 18,
            snapshot: new Uint8Array([]),
            alternateScreen: false,
            shell: { revision: 7, cwd: "/repo\nforged", phase: "prompt", lastExitCode: null },
        });
        const controller = new PtyLifecycleController(options);

        await expect(controller.attach(vi.fn())).rejects.toThrow("invalid shell metadata");
        expect(fakes.detach).toHaveBeenCalledWith(42, 18);
        expect(controller.getSnapshot()).toMatchObject({ status: "running", failureOperation: "attach" });
    });

    it("bounds pre-activation events and forces snapshot resynchronization", async () => {
        const attached = deferred<PtyAttachResult>();
        const { fakes, channels, errors, options } = controllerOptions({ maxPendingChunks: 2, maxPendingBytes: 4 });
        fakes.attach.mockReturnValue(attached.promise);
        const controller = new PtyLifecycleController(options);
        const received = vi.fn();
        const attaching = controller.attach(received);
        await vi.waitFor(() => expect(channels.bindings).toHaveLength(1));

        channels.bindings[0].emit([1, 2]);
        channels.bindings[0].emit([3, 4]);
        channels.bindings[0].emit([5]);
        attached.resolve({ subId: 10, snapshot: new Uint8Array([]), alternateScreen: false });

        await expect(attaching).rejects.toBeInstanceOf(PtySubscriptionOverflowError);
        expect(received).not.toHaveBeenCalled();
        expect(fakes.detach).toHaveBeenCalledTimes(1);
        expect(errors.some((event) => event.error instanceof PtySubscriptionOverflowError)).toBe(true);
        expect(controller.getSnapshot().attachmentCount).toBe(0);
    });

    it("bounds active chunks and total simultaneous attachments", async () => {
        const { fakes, channels, errors, options } = controllerOptions({ maxAttachments: 1, maxPendingBytes: 3 });
        const controller = new PtyLifecycleController(options);
        const listener = vi.fn();
        const attachment = await controller.attach(listener);
        attachment.activate();

        await expect(controller.attach(vi.fn())).rejects.toThrow("attachment limit");
        channels.bindings[0].emit([1, 2, 3, 4]);
        await vi.waitFor(() => expect(fakes.detach).toHaveBeenCalledOnce());

        expect(listener).not.toHaveBeenCalled();
        expect(fakes.attach).toHaveBeenCalledTimes(1);
        expect(errors.some((event) => event.error instanceof PtySubscriptionOverflowError)).toBe(true);
    });

    it("unsubscribes a valid native subscription when its snapshot is oversized", async () => {
        const { fakes, options } = controllerOptions({ maxSnapshotBytes: 2 });
        fakes.attach.mockResolvedValue({ subId: 23, snapshot: new Uint8Array([1, 2, 3]), alternateScreen: false });
        const controller = new PtyLifecycleController(options);

        await expect(controller.attach(vi.fn())).rejects.toThrow("oversized snapshot");
        expect(fakes.detach).toHaveBeenCalledOnce();
        expect(fakes.detach).toHaveBeenCalledWith(42, 23);
        expect(fakes.kill).not.toHaveBeenCalled();
        expect(controller.getSnapshot()).toMatchObject({ status: "running", attachmentCount: 0, failureOperation: "attach" });
    });

    it("contains listener failures and detaches after a bounded error budget", async () => {
        const { fakes, channels, errors, options } = controllerOptions({ maxListenerErrors: 2 });
        const controller = new PtyLifecycleController(options);
        const listenerError = new Error("renderer failed");
        const listener = vi.fn(() => {
            throw listenerError;
        });
        const attachment = await controller.attach(listener);
        attachment.activate();

        expect(() => channels.bindings[0].emit([1])).not.toThrow();
        expect(() => channels.bindings[0].emit([2])).not.toThrow();
        expect(() => channels.bindings[0].emit([3])).not.toThrow();
        await vi.waitFor(() => expect(fakes.detach).toHaveBeenCalledOnce());

        expect(listener).toHaveBeenCalledTimes(2);
        expect(errors.filter((event) => event.operation === "output-listener")).toHaveLength(2);
        expect(controller.getSnapshot().attachmentCount).toBe(0);
    });

    it("treats a zero-length native event as terminal exit", async () => {
        const { fakes, channels, options } = controllerOptions();
        const controller = new PtyLifecycleController(options);
        const listener = vi.fn();
        const attachment = await controller.attach(listener);
        attachment.activate();
        channels.bindings[0].emit([]);

        expect(listener).toHaveBeenCalledWith([]);
        expect(controller.getSnapshot()).toMatchObject({ status: "exited", failureOperation: "exit" });
        await expect(controller.write("ignored")).rejects.toBeInstanceOf(PtyControllerExitedError);
        await expect(controller.start()).rejects.toBeInstanceOf(PtyControllerExitedError);
        expect(fakes.spawn).toHaveBeenCalledOnce();
    });

    it("retries a typed attach failure without respawning or killing the live PTY", async () => {
        const attachError = Object.freeze({ category: "pty", message: "temporary attach failure" });
        const { fakes, errors, options } = controllerOptions();
        fakes.attach
            .mockRejectedValueOnce(attachError)
            .mockResolvedValueOnce({ subId: 19, snapshot: new Uint8Array([4, 5]), alternateScreen: false });
        const controller = new PtyLifecycleController(options);

        await expect(controller.attach(vi.fn())).rejects.toBe(attachError);
        expect(controller.getSnapshot()).toMatchObject({ status: "running", failureOperation: "attach", attachmentCount: 0 });

        const attachment = await controller.attach(vi.fn());
        expect(attachment.snapshot).toEqual(new Uint8Array([4, 5]));
        expect(controller.getSnapshot()).toMatchObject({ status: "running", failureOperation: null, attachmentCount: 1 });
        expect(fakes.spawn).toHaveBeenCalledOnce();
        expect(fakes.attach).toHaveBeenCalledTimes(2);
        expect(fakes.kill).not.toHaveBeenCalled();
        expect(fakes.detach).not.toHaveBeenCalled();
        expect(errors).toContainEqual({ operation: "attach", error: attachError });
    });

    it("makes an explicit native PTY lookup miss terminal", async () => {
        const attachError = Object.freeze({ category: "bad-arg", message: "invalid argument: pty not found" });
        const { fakes, errors, options } = controllerOptions();
        fakes.attach.mockRejectedValue(attachError);
        const controller = new PtyLifecycleController(options);

        await expect(controller.attach(vi.fn())).rejects.toBe(attachError);
        expect(controller.getSnapshot()).toMatchObject({ status: "exited", failureOperation: "attach", attachmentCount: 0 });
        await expect(controller.start()).rejects.toBeInstanceOf(PtyControllerExitedError);
        expect(fakes.spawn).toHaveBeenCalledOnce();
        expect(fakes.kill).not.toHaveBeenCalled();
        expect(errors).toContainEqual({ operation: "attach", error: attachError });
    });

    it("detaches subscriptions and kills once on repeated disposal", async () => {
        const { fakes, channels, options } = controllerOptions();
        const controller = new PtyLifecycleController(options);
        const first = await controller.attach(vi.fn());
        const second = await controller.attach(vi.fn());
        first.activate();
        second.activate();

        const disposing = controller.dispose();
        expect(controller.dispose()).toBe(disposing);
        await disposing;

        expect(fakes.detach).toHaveBeenCalledTimes(2);
        expect(fakes.kill).toHaveBeenCalledTimes(1);
        expect(channels.bindings.every((binding) => binding.close.mock.calls.length === 1)).toBe(true);
        expect(controller.getSnapshot()).toMatchObject({ status: "disposed", attachmentCount: 0 });
    });

    it("closes an in-flight attachment exactly once when disposal wins", async () => {
        const nativeAttach = deferred<PtyAttachResult>();
        const { fakes, channels, options } = controllerOptions();
        fakes.attach.mockReturnValue(nativeAttach.promise);
        const controller = new PtyLifecycleController(options);
        const attaching = controller.attach(vi.fn());
        await vi.waitFor(() => expect(channels.bindings).toHaveLength(1));
        const disposing = controller.dispose();
        nativeAttach.reject(new Error("killed while attaching"));

        await expect(attaching).rejects.toBeInstanceOf(PtyControllerDisposedError);
        await disposing;
        expect(channels.bindings[0].close).toHaveBeenCalledOnce();
        expect(fakes.kill).toHaveBeenCalledOnce();
        expect(fakes.detach).not.toHaveBeenCalled();
    });
});

describe("PtyLifecycleController safety boundaries", () => {
    it("strictly parses scalar shell metadata without invoking accessors", () => {
        expect(parsePtyShellMetadataSnapshot({ revision: 1, cwd: null, phase: "unknown", lastExitCode: null })).toEqual({
            revision: 1,
            cwd: null,
            phase: "unknown",
            lastExitCode: null,
        });
        expect(parsePtyShellMetadataSnapshot({ revision: -1, cwd: "/repo", phase: "prompt", lastExitCode: null })).toBeNull();
        expect(parsePtyShellMetadataSnapshot({ revision: 1, cwd: "/repo", phase: "forged", lastExitCode: null })).toBeNull();
        expect(
            parsePtyShellMetadataSnapshot(
                Object.defineProperty({ revision: 1, cwd: "/repo", lastExitCode: null }, "phase", {
                    enumerable: true,
                    get: () => {
                        throw new Error("must not run");
                    },
                }),
            ),
        ).toBeNull();
    });

    it("exposes content-free runtime snapshots and rejects persistence", async () => {
        const timer = new FakeTimer();
        const { options } = controllerOptions({ initialInput: "secret first task", timer });
        const controller = new PtyLifecycleController(options);
        await controller.start();

        const snapshotText = JSON.stringify(controller.getSnapshot());
        expect(snapshotText).not.toContain("secret first task");
        expect(snapshotText).not.toContain("42");
        expect(() => JSON.stringify(controller)).toThrow("cannot be serialized");

        const attachment = await controller.attach(vi.fn());
        expect(() => JSON.stringify(attachment)).toThrow("cannot be serialized");
    });

    it("bounds state subscribers and removes listeners that throw", async () => {
        const observerError = new Error("observer failed");
        const { errors, options } = controllerOptions({ maxStateListeners: 1 });
        const controller = new PtyLifecycleController(options);
        const unsubscribe = controller.subscribe(() => {
            throw observerError;
        });
        unsubscribe();
        const listener = vi.fn();
        controller.subscribe(listener);
        expect(() => controller.subscribe(vi.fn())).toThrow(RangeError);

        await controller.start();
        expect(listener).toHaveBeenCalled();
        expect(errors).toContainEqual({ operation: "state-listener", error: observerError });
    });

    it("validates native dimensions and externally supplied runtime IDs", async () => {
        const { fakes, options } = controllerOptions();
        const controller = new PtyLifecycleController(options);

        expect(() => new PtyLifecycleController({ ...options, existingPtyId: -1 })).toThrow(RangeError);
        expect(() => new PtyLifecycleController({ ...options, existingPtyId: Number.MAX_SAFE_INTEGER + 1 })).toThrow(RangeError);
        expect(() => new PtyLifecycleController({ ...options, existingPtyId: 1.5 })).toThrow(RangeError);
        expect(() => new PtyLifecycleController({ ...options, existingPtyId: 1_759_300_000_123 })).not.toThrow();
        await expect(controller.resize(0, 24)).rejects.toThrow(RangeError);
        await expect(controller.resize(80, 65_536)).rejects.toThrow(RangeError);
        await expect(controller.adoptExistingPty(42)).rejects.toThrow("controller-owned PTY");
        expect(fakes.spawn).not.toHaveBeenCalled();
        expect(fakes.resize).not.toHaveBeenCalled();
    });
});
