import { describe, expect, it, vi } from "vitest";
import {
    DuplicateExtensionContributionError,
    DuplicateInternalExtensionError,
    INTERNAL_EXTENSION_HOST_LIMITS,
    InternalExtensionHost,
    InternalExtensionHostDisposedError,
    createInternalExtensionId,
    type ExtensionContributionAdapter,
    type ExtensionContributionContext,
    type InternalExtensionHostAdapters,
    type InternalExtensionHostOptions,
} from "./host";

interface TestContribution {
    readonly label: string;
    readonly secret?: string;
    readonly registerError?: Error;
    readonly invalidRegistration?: boolean;
    readonly disposeError?: Error;
}

type TestAdapters = InternalExtensionHostAdapters<TestContribution, TestContribution, TestContribution>;

function adapters(events: string[] = []): TestAdapters & { readonly contexts: ExtensionContributionContext[] } {
    const contexts: ExtensionContributionContext[] = [];
    const adapter = (kind: string): ExtensionContributionAdapter<TestContribution> => ({
        register: vi.fn((value, context) => {
            contexts.push(context);
            events.push(`register:${kind}:${value.label}`);
            if (value.registerError) throw value.registerError;
            if (value.invalidRegistration) return null as never;
            return {
                dispose: () => {
                    events.push(`dispose:${kind}:${value.label}`);
                    if (value.disposeError) throw value.disposeError;
                },
            };
        }),
    });
    return {
        actions: adapter("action"),
        workbenchItems: adapter("workbench-item"),
        taskProviders: adapter("task-provider"),
        contexts,
    };
}

function host(testAdapters = adapters(), options: InternalExtensionHostOptions = {}) {
    return new InternalExtensionHost<TestContribution, TestContribution, TestContribution>(testAdapters, options);
}

function contribution(label: string, overrides: Partial<TestContribution> = {}) {
    return () => ({ label, ...overrides });
}

describe("InternalExtensionHost registration", () => {
    it("routes bounded namespaced contributions through injected adapters", () => {
        const testAdapters = adapters();
        const extensionHost = host(testAdapters);
        const registration = extensionHost.register({
            id: "sikemux.tools",
            actions: [{ id: "pane.close", create: contribution("close", { secret: "action payload" }) }],
            workbenchItems: [{ id: "notes", create: contribution("notes", { secret: "item payload" }) }],
            taskProviders: [{ id: "cargo", create: contribution("cargo", { secret: "task payload" }) }],
        });

        expect(testAdapters.contexts).toEqual([
            {
                extensionId: "sikemux.tools",
                contributionId: "internal:sikemux.tools/action/pane.close",
                kind: "action",
                localId: "pane.close",
            },
            {
                extensionId: "sikemux.tools",
                contributionId: "internal:sikemux.tools/workbench-item/notes",
                kind: "workbench-item",
                localId: "notes",
            },
            {
                extensionId: "sikemux.tools",
                contributionId: "internal:sikemux.tools/task-provider/cargo",
                kind: "task-provider",
                localId: "cargo",
            },
        ]);
        expect(testAdapters.contexts.every(Object.isFrozen)).toBe(true);
        expect(registration).toMatchObject({ id: "sikemux.tools", disposed: false, activeContributions: 3 });
        expect(Object.isFrozen(registration)).toBe(true);

        const snapshot = extensionHost.getSnapshot();
        expect(snapshot).toMatchObject({
            disposed: false,
            extensionCount: 1,
            declaredContributions: 3,
            activeContributions: 3,
            contributionCounts: { actions: 1, workbenchItems: 1, taskProviders: 1 },
            failureCount: 0,
        });
        expect(snapshot.extensions[0]).toMatchObject({
            id: "sikemux.tools",
            declaredContributions: 3,
            activeContributions: 3,
        });
        expect(Object.isFrozen(snapshot)).toBe(true);
        expect(Object.isFrozen(snapshot.extensions)).toBe(true);
        expect(Object.isFrozen(snapshot.extensions[0])).toBe(true);
        expect(Object.isFrozen(snapshot.extensions[0].contributionCounts)).toBe(true);
        expect(JSON.stringify(snapshot)).not.toContain("payload");
    });

    it.each(["plain", "Upper.case", "constructor.tools", "sikemux.__proto__", `sikemux.${"a".repeat(128)}`, "sikemux.bad\u0000id"])(
        "rejects invalid namespaced extension ID: %s",
        (id) => {
            expect(() => createInternalExtensionId(id)).toThrow(TypeError);
        },
    );

    it("rejects extension and same-kind contribution duplicates before activation", () => {
        const testAdapters = adapters();
        const extensionHost = host(testAdapters);

        expect(() =>
            extensionHost.register({
                id: "sikemux.duplicate-contribution",
                actions: [
                    { id: "same", create: contribution("one") },
                    { id: "same", create: contribution("two") },
                ],
            }),
        ).toThrow(DuplicateExtensionContributionError);
        expect(testAdapters.contexts).toEqual([]);
        expect(extensionHost.getSnapshot().extensionCount).toBe(0);

        extensionHost.register({ id: "sikemux.unique", actions: [{ id: "same", create: contribution("action") }] });
        expect(() => extensionHost.register({ id: "sikemux.unique" })).toThrow(DuplicateInternalExtensionError);
        expect(() =>
            extensionHost.register({
                id: "sikemux.invalid-contribution",
                actions: [{ id: "constructor.run", create: contribution("unsafe") }],
            }),
        ).toThrow(TypeError);
    });
});

describe("InternalExtensionHost failure containment", () => {
    it("contains predicate, factory, and adapter failures while activating healthy siblings", async () => {
        const hostileMessage = vi.fn(() => "do not read getter");
        const hostileError = {};
        Object.defineProperty(hostileError, "message", { get: hostileMessage });
        const skippedFactory = vi.fn(contribution("skipped"));
        const extensionHost = host();

        const registration = extensionHost.register({
            id: "sikemux.containment",
            actions: [
                { id: "skipped", when: () => false, create: skippedFactory },
                {
                    id: "predicate-throws",
                    when: () => {
                        throw new Error("predicate\u0000failed");
                    },
                    create: contribution("never"),
                },
                {
                    id: "predicate-async",
                    when: (() => Promise.reject(new Error("async predicate"))) as unknown as () => boolean,
                    create: contribution("never"),
                },
                {
                    id: "factory-throws",
                    create: () => {
                        throw hostileError;
                    },
                },
                {
                    id: "factory-async",
                    create: (() => Promise.reject(new Error("async factory"))) as unknown as () => TestContribution,
                },
                { id: "adapter-throws", create: contribution("bad adapter", { registerError: new Error("adapter failed") }) },
                { id: "invalid-registration", create: contribution("invalid registration", { invalidRegistration: true }) },
                { id: "healthy", create: contribution("healthy") },
            ],
        });
        await Promise.resolve();

        expect(skippedFactory).not.toHaveBeenCalled();
        expect(hostileMessage).not.toHaveBeenCalled();
        expect(registration.activeContributions).toBe(1);
        const snapshot = extensionHost.getSnapshot();
        expect(snapshot).toMatchObject({ declaredContributions: 8, activeContributions: 1, failureCount: 6 });
        expect(snapshot.failures.map(({ stage }) => stage)).toEqual(["predicate", "predicate", "factory", "factory", "register", "register"]);
        expect(snapshot.failures[0].message).toBe("predicate failed");
        expect(snapshot.failures[2].message).toBe("Internal extension contribution failed");
        expect(snapshot.failures[4]).toMatchObject({
            contributionId: "internal:sikemux.containment/action/adapter-throws",
            message: "adapter failed",
        });
    });

    it("bounds, sanitizes, and freezes scalar-only failure history", () => {
        const extensionHost = host(adapters(), { maxFailureHistory: 2 });
        extensionHost.register({
            id: "sikemux.failure-ring",
            actions: ["one", "two", "three"].map((id) => ({
                id,
                create: () => {
                    throw `${id}:${"x".repeat(INTERNAL_EXTENSION_HOST_LIMITS.maxFailureMessageLength + 10)}\u0000`;
                },
            })),
        });

        const snapshot = extensionHost.getSnapshot();
        expect(snapshot.failureCount).toBe(3);
        expect(snapshot.failures).toHaveLength(2);
        expect(snapshot.failures.map(({ sequence }) => sequence)).toEqual([2, 3]);
        expect(snapshot.failures.every(({ message }) => message.length <= INTERNAL_EXTENSION_HOST_LIMITS.maxFailureMessageLength)).toBe(true);
        expect(snapshot.failures.every(({ message }) => !message.includes("\u0000"))).toBe(true);
        expect(Object.isFrozen(snapshot.failures)).toBe(true);
        expect(Object.isFrozen(snapshot.failures[0])).toBe(true);
    });
});

describe("InternalExtensionHost revocation", () => {
    it("revokes every contribution in reverse order and contains disposer errors", () => {
        const events: string[] = [];
        const extensionHost = host(adapters(events));
        const registration = extensionHost.register({
            id: "sikemux.revoke",
            actions: [{ id: "first", create: contribution("first") }],
            workbenchItems: [{ id: "second", create: contribution("second", { disposeError: new Error("second dispose failed") }) }],
            taskProviders: [{ id: "third", create: contribution("third") }],
        });
        events.length = 0;

        registration.dispose();
        registration.dispose();

        expect(events).toEqual(["dispose:task-provider:third", "dispose:workbench-item:second", "dispose:action:first"]);
        expect(registration).toMatchObject({ disposed: true, activeContributions: 0 });
        expect(extensionHost.getSnapshot()).toMatchObject({
            disposed: false,
            extensionCount: 0,
            declaredContributions: 0,
            activeContributions: 0,
            failureCount: 1,
        });
        expect(extensionHost.getSnapshot().failures[0]).toMatchObject({ stage: "dispose", message: "second dispose failed" });

        const replacement = extensionHost.register({ id: "sikemux.revoke" });
        expect(replacement.disposed).toBe(false);
    });

    it("disposes extensions newest-first, stays idempotent, and rejects late registration", () => {
        const events: string[] = [];
        const extensionHost = host(adapters(events));
        extensionHost.register({ id: "sikemux.first", actions: [{ id: "action", create: contribution("first") }] });
        extensionHost.register({ id: "sikemux.second", taskProviders: [{ id: "tasks", create: contribution("second") }] });
        events.length = 0;

        extensionHost.dispose();
        extensionHost.dispose();

        expect(events).toEqual(["dispose:task-provider:second", "dispose:action:first"]);
        expect(extensionHost.getSnapshot()).toMatchObject({ disposed: true, extensionCount: 0, activeContributions: 0 });
        expect(() => extensionHost.register({ id: "sikemux.late" })).toThrow(InternalExtensionHostDisposedError);
    });

    it("immediately revokes a registration returned after reentrant host disposal", () => {
        const dispose = vi.fn();
        const state: { host?: InternalExtensionHost<TestContribution, TestContribution, TestContribution> } = {};
        const actionRegister = vi.fn(() => {
            state.host?.dispose();
            return { dispose };
        });
        const passive: ExtensionContributionAdapter<TestContribution> = { register: () => ({ dispose: vi.fn() }) };
        const extensionHost = new InternalExtensionHost<TestContribution, TestContribution, TestContribution>({
            actions: { register: actionRegister },
            workbenchItems: passive,
            taskProviders: passive,
        });
        state.host = extensionHost;

        const registration = extensionHost.register({
            id: "sikemux.reentrant",
            actions: [
                { id: "first", create: contribution("first") },
                { id: "never", create: contribution("never") },
            ],
        });

        expect(actionRegister).toHaveBeenCalledOnce();
        expect(dispose).toHaveBeenCalledOnce();
        expect(registration).toMatchObject({ disposed: true, activeContributions: 0 });
        expect(extensionHost.getSnapshot()).toMatchObject({ disposed: true, extensionCount: 0, activeContributions: 0 });
    });
});
