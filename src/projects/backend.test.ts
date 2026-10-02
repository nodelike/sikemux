import { describe, expect, it, vi } from "vitest";
import {
    LocalProjectBackend,
    ProjectBackendDisposedError,
    ProjectBackendRegistry,
    UnsupportedCapabilityError,
    createProjectCapabilities,
    createProjectLocation,
    projectKey,
    type ProjectBackend,
    type ProjectBackendCallOptions,
    type ProjectBackendRequest,
    type ProjectCapability,
    type ProjectLocation,
    type ProjectScheme,
} from "./backend";

function deferred<T>() {
    let resolve!: (value: T | PromiseLike<T>) => void;
    let reject!: (reason?: unknown) => void;
    const promise = new Promise<T>((resolvePromise, rejectPromise) => {
        resolve = resolvePromise;
        reject = rejectPromise;
    });
    return { promise, resolve, reject };
}

const LOCAL = createProjectLocation({ scheme: "local", path: "/workspace/project" });
const SSH = createProjectLocation({ scheme: "ssh", host: "server.example", path: "/workspace/project", user: "dev", port: 2222 });
const REQUEST = Object.freeze({ operation: "list", input: Object.freeze({ depth: 2 }) });

function fakeBackend(scheme: ProjectScheme, enabled: readonly ProjectCapability[], result: unknown) {
    const invoke = vi.fn(async (_location: ProjectLocation, _request: ProjectBackendRequest, _options?: ProjectBackendCallOptions) => result);
    const dispose = vi.fn(async () => {});
    const backend = {
        scheme,
        capabilities: createProjectCapabilities(enabled),
        files: invoke,
        watch: invoke,
        pty: invoke,
        lsp: invoke,
        git: invoke,
        tasks: invoke,
        dispose,
    } satisfies ProjectBackend;
    return { backend, invoke, dispose };
}

describe("ProjectLocation validation and identity", () => {
    it("copies, canonicalizes, and freezes local and SSH locations", () => {
        const localInput = { scheme: "local", path: "/repo" };
        const local = createProjectLocation(localInput);
        localInput.path = "/changed";

        const ssh = createProjectLocation({
            scheme: "ssh",
            host: "[2001:DB8::1]",
            path: "/srv/repo",
            user: "deploy",
            port: 2222,
        });

        expect(local).toEqual({ scheme: "local", path: "/repo" });
        expect(ssh).toEqual({ scheme: "ssh", host: "2001:db8::1", path: "/srv/repo", user: "deploy", port: 2222 });
        expect(Object.isFrozen(local)).toBe(true);
        expect(Object.isFrozen(ssh)).toBe(true);
    });

    it("rejects relative paths, unsafe fields, and invalid SSH authority", () => {
        expect(() => createProjectLocation({ scheme: "local", path: "relative/repo" })).toThrow("absolute");
        expect(() => createProjectLocation({ scheme: "ssh", host: "host", path: "relative" })).toThrow("absolute");
        expect(() => createProjectLocation({ scheme: "ssh", host: "[::1", path: "/repo" })).toThrow("balanced");
        expect(() => createProjectLocation({ scheme: "ssh", host: "::::", path: "/repo" })).toThrow("invalid");
        expect(() => createProjectLocation({ scheme: "ssh", host: "host", path: "/repo", port: 0 })).toThrow(RangeError);
        expect(() => createProjectLocation({ scheme: "ssh", host: "host", path: "/repo", user: "bad user" })).toThrow("invalid");
        expect(() => createProjectLocation({ scheme: "local", path: "/repo", token: "secret" })).toThrow("Unknown");
        expect(() => createProjectLocation({ scheme: "other", path: "/repo" })).toThrow("local or ssh");
    });

    it("builds canonical collision-free keys across users, ports, schemes, and IPv6 spelling", () => {
        const ipv6 = createProjectLocation({ scheme: "ssh", host: "2001:DB8::1", path: "/srv/repo", user: "deploy", port: 2222 });
        const bracketed = createProjectLocation({ scheme: "ssh", host: "[2001:db8::1]", path: "/srv/repo", user: "deploy", port: 2222 });
        expect(projectKey(ipv6)).toBe("ssh|u6:deploy|h11:2001:db8::1|o2222|p9:/srv/repo");
        expect(projectKey(bracketed)).toBe(projectKey(ipv6));

        const keys = new Set([
            projectKey({ scheme: "local", path: "/srv/repo" }),
            projectKey({ scheme: "ssh", host: "bc", path: "/srv/repo", user: "a" }),
            projectKey({ scheme: "ssh", host: "c", path: "/srv/repo", user: "ab" }),
            projectKey({ scheme: "ssh", host: "2001:db8::1", path: "/srv/repo", user: "deploy" }),
            projectKey({ scheme: "ssh", host: "2001:db8::1", path: "/srv/repo", user: "deploy", port: 22 }),
            projectKey({ scheme: "ssh", host: "2001:db8::1", path: "/srv/repo", user: "other", port: 22 }),
        ]);
        expect(keys.size).toBe(6);
    });
});

describe("LocalProjectBackend", () => {
    it("derives immutable capabilities, forwards validated calls, and enforces unsupported operations", async () => {
        const files = vi.fn(async (location, request, options) => ({ location, request, options }));
        const backend = new LocalProjectBackend({ files });

        expect(backend.capabilities).toEqual({ files: true, watch: false, pty: false, lsp: false, git: false, tasks: false });
        expect(Object.isFrozen(backend.capabilities)).toBe(true);
        const result = await backend.files(LOCAL, REQUEST);
        expect(result).toMatchObject({ location: LOCAL, request: REQUEST, options: {} });
        expect(Object.isFrozen(files.mock.calls[0][0])).toBe(true);
        expect(Object.isFrozen(files.mock.calls[0][1])).toBe(true);

        await expect(backend.git(LOCAL, { operation: "status" })).rejects.toMatchObject({
            name: "UnsupportedCapabilityError",
            scheme: "local",
            capability: "git",
        });
        await expect(backend.files(SSH, REQUEST)).rejects.toThrow("requires a local location");
    });

    it("propagates AbortSignal and rejects promptly without waiting for an injected operation", async () => {
        const pending = deferred<unknown>();
        const files = vi.fn((_location, _request, options) => {
            expect(options.signal).toBe(controller.signal);
            return pending.promise;
        });
        const backend = new LocalProjectBackend({ files });
        const controller = new AbortController();
        const reason = new Error("cancel project read");

        const reading = backend.files(LOCAL, REQUEST, { signal: controller.signal });
        controller.abort(reason);

        await expect(reading).rejects.toBe(reason);
        expect(files).toHaveBeenCalledOnce();
        pending.resolve("late result");

        const preAborted = new AbortController();
        preAborted.abort(reason);
        await expect(backend.files(LOCAL, REQUEST, { signal: preAborted.signal })).rejects.toBe(reason);
        expect(files).toHaveBeenCalledOnce();
    });

    it("disposes injected operations once and rejects later calls", async () => {
        const dispose = vi.fn(async () => {});
        const backend = new LocalProjectBackend({ files: async () => [], dispose });

        const first = backend.dispose();
        expect(backend.dispose()).toBe(first);
        await first;

        expect(dispose).toHaveBeenCalledOnce();
        await expect(backend.files(LOCAL, REQUEST)).rejects.toBeInstanceOf(ProjectBackendDisposedError);
    });
});

describe("ProjectBackendRegistry routing and isolation", () => {
    it("selects trusted backends by location scheme and enforces snapshotted capabilities", async () => {
        const local = fakeBackend("local", ["files"], "local-result");
        const ssh = fakeBackend("ssh", ["git"], "ssh-result");
        const registry = new ProjectBackendRegistry();
        registry.registerTrusted(local.backend);
        registry.registerTrusted(ssh.backend);
        const controller = new AbortController();

        await expect(registry.files(LOCAL, REQUEST, { signal: controller.signal })).resolves.toBe("local-result");
        await expect(registry.git(SSH, { operation: "status" })).resolves.toBe("ssh-result");
        expect(local.invoke).toHaveBeenCalledOnce();
        expect(local.invoke.mock.calls[0][2]).toEqual({ signal: controller.signal });
        expect(ssh.invoke).toHaveBeenCalledOnce();

        await expect(registry.files(SSH, REQUEST)).rejects.toBeInstanceOf(UnsupportedCapabilityError);
        expect(ssh.invoke).toHaveBeenCalledOnce();
    });

    it("bounds registrations and disposes an unregistered backend exactly once", async () => {
        const local = fakeBackend("local", ["files"], "local");
        const ssh = fakeBackend("ssh", ["files"], "ssh");
        const registry = new ProjectBackendRegistry({ maxRegistrations: 1 });
        const registration = registry.registerTrusted(local.backend);

        expect(() => registry.registerTrusted(ssh.backend)).toThrow(RangeError);
        const first = registration.dispose();
        expect(registration.dispose()).toBe(first);
        await first;
        expect(local.dispose).toHaveBeenCalledOnce();
        expect(() => registry.resolve(LOCAL)).toThrow("No trusted project backend");
    });
});
