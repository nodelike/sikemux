import { describe, expect, it, vi } from "vitest";
import {
    ACTION_REGISTRY_LIMITS,
    ActionDisabledError,
    ActionNotFoundError,
    ActionNotVisibleError,
    ActionRegistry,
    ActionRegistryDisposedError,
    DuplicateActionContributionError,
    createActionContext,
    fingerprintActionContext,
    type ActionContext,
    type ActionContextInput,
    type ActionDefinition,
} from "./registry";

type Mutable<T> = { -readonly [Key in keyof T]: T[Key] };

function action<Result = string>(id: string, overrides: Partial<ActionDefinition<Result>> = {}): Mutable<ActionDefinition<Result>> {
    return {
        id,
        title: `Action ${id}`,
        detail: `Run ${id}`,
        category: "Test",
        source: "core",
        defaultBinding: null,
        run: (() => id) as unknown as ActionDefinition<Result>["run"],
        ...overrides,
    };
}

function fullContext(overrides: ActionContextInput = {}): ActionContextInput {
    return {
        focusedItem: { id: "item-1", kind: "terminal" },
        session: { id: "session-1", kind: "project" },
        project: { id: "project-1", root: "/workspace/project" },
        focus: { target: "terminal", editable: true },
        modal: { id: "modal-1", kind: "commandPalette" },
        agent: { id: "agent-1", kind: "codex", status: "running" },
        capabilities: ["agent.run", "terminal.write"],
        ...overrides,
    };
}

describe("action context", () => {
    it("creates an immutable bounded scalar context with canonical capabilities", () => {
        const capabilities = ["terminal.write", "agent.run", "terminal.write"];
        const focusedItem = { id: "item-1", kind: "terminal" };
        const context = createActionContext(fullContext({ capabilities, focusedItem }));
        capabilities.push("project.task");
        focusedItem.kind = "editor";

        expect(context).toEqual({
            focusedItem: { id: "item-1", kind: "terminal" },
            session: { id: "session-1", kind: "project" },
            project: { id: "project-1", root: "/workspace/project" },
            focus: { target: "terminal", editable: true },
            modal: { id: "modal-1", kind: "commandPalette" },
            agent: { id: "agent-1", kind: "codex", status: "running" },
            capabilities: ["agent.run", "terminal.write"],
        });
        expect(Object.isFrozen(context)).toBe(true);
        expect(Object.isFrozen(context.focusedItem)).toBe(true);
        expect(Object.isFrozen(context.capabilities)).toBe(true);
        expect(createActionContext()).toEqual({
            focusedItem: null,
            session: null,
            project: null,
            focus: { target: "application", editable: false },
            modal: null,
            agent: null,
            capabilities: [],
        });
    });

    it("fingerprints every fixed context dimension without capability-order noise", () => {
        const first = fingerprintActionContext(fullContext({ capabilities: ["terminal.write", "agent.run"] }));
        const reordered = fingerprintActionContext(fullContext({ capabilities: ["agent.run", "terminal.write"] }));
        const changedFocus = fingerprintActionContext(fullContext({ focus: { target: "editor", editable: true } }));
        const changedModal = fingerprintActionContext(fullContext({ modal: null }));
        const delimiterLikeId = fingerprintActionContext(fullContext({ focusedItem: { id: 'item-1","session-1', kind: "terminal" } }));

        expect(first).toBe(reordered);
        expect(changedFocus).not.toBe(first);
        expect(changedModal).not.toBe(first);
        expect(delimiterLikeId).not.toBe(first);
        expect(JSON.parse(first)[0]).toBe("action-context-v1");
    });

    it.each([
        ["unsafe focused item ID", { focusedItem: { id: "__proto__", kind: "terminal" } }],
        ["control-bearing session ID", { session: { id: "session\n1", kind: "project" } }],
        ["unsafe session kind", { session: { id: "session-1", kind: "constructor" } }],
        ["oversized project ID", { project: { id: "x".repeat(ACTION_REGISTRY_LIMITS.maxEntityIdLength + 1), root: "/ok" } }],
        ["control-bearing project root", { project: { id: "project-1", root: "/bad\nroot" } }],
        ["unsafe focus target", { focus: { target: "prototype", editable: false } }],
        ["control-bearing modal kind", { modal: { id: "modal-1", kind: "bad\tkind" } }],
        ["unsafe agent status", { agent: { id: "agent-1", kind: "codex", status: "__proto__" } }],
        ["unsafe capability", { capabilities: ["terminal.write", "core.constructor"] }],
    ])("rejects malformed context data: %s", (_label, input) => {
        expect(() => createActionContext(input as ActionContextInput)).toThrow(TypeError);
    });

    it("bounds capability enumeration even when values repeat", () => {
        const capabilities = Array.from({ length: ACTION_REGISTRY_LIMITS.maxCapabilities + 1 }, () => "terminal.write");
        expect(() => createActionContext({ capabilities })).toThrow(RangeError);
    });
});

describe("ActionRegistry registration", () => {
    it("creates namespaced contribution IDs and rejects ambiguous resolution slots", () => {
        const registry = new ActionRegistry();
        const global = registry.register(action("pane.close"));
        const item = registry.register(action("pane.close", { source: "terminal.actions" }), {
            scope: { kind: "focused-item", id: "item-1" },
        });
        const otherItem = registry.register(action("pane.close", { source: "editor.actions" }), {
            scope: { kind: "focused-item", id: "item-2" },
        });

        expect(global.contributionId).toBe("internal:core/pane.close@global");
        expect(item.contributionId).toBe('internal:terminal.actions/pane.close@focused-item:"item-1"');
        expect(otherItem.contributionId).not.toBe(item.contributionId);
        expect(() =>
            registry.register(action("pane.close", { source: "other.source" }), {
                scope: { kind: "focused-item", id: "item-1" },
            }),
        ).toThrow(DuplicateActionContributionError);

        try {
            registry.register(action("pane.close", { source: "other.source" }));
        } catch (error) {
            expect(error).toMatchObject({
                actionId: "pane.close",
                scope: "global",
                targetId: null,
                existingContributionId: global.contributionId,
            });
        }
    });

    it("returns idempotent disposable registrations and stable ordered metadata", () => {
        const registry = new ActionRegistry();
        const cleaned = vi.fn();
        const mutable = action("first.action");
        const first = registry.register(mutable, { onDispose: cleaned });
        const second = registry.register(action("second.action"));
        mutable.title = "mutated after registration";

        expect(registry.size).toBe(2);
        expect(registry.contributions().map(({ definition }) => definition.title)).toEqual(["Action first.action", "Action second.action"]);
        expect(first.registrationOrder).toBeLessThan(second.registrationOrder);
        expect(first.disposed).toBe(false);

        first.dispose();
        first.dispose();
        expect(first.disposed).toBe(true);
        expect(cleaned).toHaveBeenCalledOnce();
        expect(registry.size).toBe(1);

        const replacement = registry.register(action("first.action"));
        expect(replacement.registrationOrder).toBeGreaterThan(second.registrationOrder);
    });

    it.each([
        ["unsafe action ID", action("__proto__")],
        ["unsafe action ID segment", action("pane.constructor")],
        ["oversized action ID", action(`a${"x".repeat(ACTION_REGISTRY_LIMITS.maxIdLength)}`)],
        ["control-bearing title", action("valid.action", { title: "Bad\ntitle" })],
        ["blank title", action("valid.action", { title: "" })],
        ["oversized detail", action("valid.action", { detail: "x".repeat(ACTION_REGISTRY_LIMITS.maxDetailLength + 1) })],
        ["unsafe category", action("valid.action", { category: "constructor" })],
        ["unsafe source", action("valid.action", { source: "project.__proto__" })],
        ["control-bearing binding", action("valid.action", { defaultBinding: "Ctrl+K\nCtrl+S" })],
    ])("rejects malformed bounded action fields: %s", (_label, definition) => {
        expect(() => new ActionRegistry().register(definition)).toThrow(TypeError);
    });

    it("rejects malformed callbacks, scopes, and cleanup hooks at runtime", () => {
        const registry = new ActionRegistry();
        expect(() => registry.register({ ...action("bad.when"), when: true } as unknown as ActionDefinition)).toThrow(TypeError);
        expect(() => registry.register({ ...action("bad.enabled"), enabled: "yes" } as unknown as ActionDefinition)).toThrow(TypeError);
        expect(() => registry.register({ ...action("bad.run"), run: null } as unknown as ActionDefinition)).toThrow(TypeError);
        expect(() => registry.register(action("bad.scope"), { scope: { kind: "window", id: "window-1" } as never })).toThrow(TypeError);
        expect(() => registry.register(action("bad.target"), { scope: { kind: "project", id: "constructor" } })).toThrow(TypeError);
        expect(() => registry.register(action("bad.cleanup"), { onDispose: "cleanup" as never })).toThrow(TypeError);
    });

    it("enforces a finite contribution capacity", () => {
        const registry = new ActionRegistry();
        for (let index = 0; index < ACTION_REGISTRY_LIMITS.maxRegistrations; index += 1) {
            registry.register(action(`task${index}`));
        }
        expect(registry.size).toBe(ACTION_REGISTRY_LIMITS.maxRegistrations);
        expect(() => registry.register(action("overflow"))).toThrow(RangeError);
    });
});

describe("ActionRegistry contextual resolution", () => {
    it("resolves focused-item, session, project, then global precedence", async () => {
        const registry = new ActionRegistry();
        let itemVisible = true;
        let sessionVisible = true;
        let sessionEnabled = true;
        registry.register(action("task.run", { source: "core", run: () => "global" }));
        registry.register(action("task.run", { source: "project.tasks", run: () => "project" }), {
            scope: { kind: "project", id: "project-1" },
        });
        registry.register(
            action("task.run", {
                source: "session.tasks",
                when: () => sessionVisible,
                enabled: () => sessionEnabled,
                run: () => "session",
            }),
            { scope: { kind: "session", id: "session-1" } },
        );
        registry.register(action("task.run", { source: "terminal.tasks", when: () => itemVisible, run: () => "item" }), {
            scope: { kind: "focused-item", id: "item-1" },
        });

        expect(registry.resolveAction("task.run", fullContext())).toMatchObject({
            definition: { source: "terminal.tasks" },
            visible: true,
            enabled: true,
            precedence: {
                scope: "focused-item",
                rank: 3,
                targetId: "item-1",
                matchingContributions: 4,
                fallbackDepth: 0,
                shadowedContributions: 3,
            },
        });
        expect(await registry.execute("task.run", fullContext())).toBe("item");

        itemVisible = false;
        const sameFingerprint = registry.resolveAction("task.run", fullContext());
        expect(sameFingerprint).toMatchObject({
            definition: { source: "session.tasks" },
            visible: true,
            enabled: true,
            precedence: { scope: "session", fallbackDepth: 1, shadowedContributions: 2 },
        });
        expect(await registry.execute("task.run", fullContext())).toBe("session");

        sessionEnabled = false;
        expect(registry.resolveAction("task.run", fullContext())).toMatchObject({
            definition: { source: "session.tasks" },
            visible: true,
            enabled: false,
        });
        await expect(registry.execute("task.run", fullContext())).rejects.toBeInstanceOf(ActionDisabledError);

        sessionVisible = false;
        expect(await registry.execute("task.run", fullContext())).toBe("project");
        expect(await registry.execute("task.run", fullContext({ focusedItem: null, session: null, project: null }))).toBe("global");
    });

    it("lists visible and disabled actions deterministically and can inspect hidden ones", () => {
        const registry = new ActionRegistry();
        registry.register(action("first.visible"));
        registry.register(action("second.hidden", { when: () => false }));
        registry.register(action("third.disabled", { enabled: () => false }));
        registry.register(action("other.project"), { scope: { kind: "project", id: "project-2" } });

        expect(registry.resolve(fullContext()).map(({ definition, enabled }) => [definition.id, enabled])).toEqual([
            ["first.visible", true],
            ["third.disabled", false],
        ]);
        expect(registry.resolveAction("missing.action", fullContext())).toBeUndefined();
        expect(registry.resolveAction("other.project", fullContext())).toBeUndefined();
    });

    it("propagates predicate failures and rejects non-boolean predicate results", () => {
        const registry = new ActionRegistry();
        const failure = new Error("predicate failed");
        registry.register(
            action("broken.when", {
                when: () => {
                    throw failure;
                },
            }),
        );
        registry.register(action("broken.enabled", { enabled: (() => "yes") as unknown as () => boolean }));

        expect(() => registry.resolveAction("broken.when", {})).toThrow(failure);
        expect(() => registry.resolveAction("broken.enabled", {})).toThrow(TypeError);
    });
});

describe("ActionRegistry execution and teardown", () => {
    it("preserves synchronous and asynchronous results and raw errors", async () => {
        const registry = new ActionRegistry();
        const result = { taskId: "task-1" };
        const asyncFailure = new Error("async failure");
        const syncFailure = { category: "task", reason: "sync failure" };
        let receivedContext: ActionContext | undefined;
        registry.register(
            action<typeof result>("result.identity", {
                run: (context) => {
                    receivedContext = context;
                    return result;
                },
            }),
        );
        registry.register(action("error.async", { run: () => Promise.reject(asyncFailure) }));
        registry.register(
            action("error.sync", {
                run: () => {
                    throw syncFailure;
                },
            }),
        );

        await expect(registry.execute<typeof result>("result.identity", fullContext())).resolves.toBe(result);
        expect(receivedContext?.capabilities).toEqual(["agent.run", "terminal.write"]);
        expect(Object.isFrozen(receivedContext)).toBe(true);
        await expect(registry.execute("error.async", {})).rejects.toBe(asyncFailure);
        await expect(registry.execute("error.sync", {})).rejects.toBe(syncFailure);
    });

    it("uses explicit not-found, hidden, and disabled dispatch errors without running", async () => {
        const registry = new ActionRegistry();
        const hiddenRun = vi.fn();
        const disabledRun = vi.fn();
        registry.register(action("hidden.action", { when: () => false, run: hiddenRun }));
        registry.register(action("disabled.action", { enabled: () => false, run: disabledRun }));

        await expect(registry.execute("missing.action", {})).rejects.toBeInstanceOf(ActionNotFoundError);
        await expect(registry.execute("hidden.action", {})).rejects.toBeInstanceOf(ActionNotVisibleError);
        await expect(registry.execute("disabled.action", {})).rejects.toBeInstanceOf(ActionDisabledError);
        expect(hiddenRun).not.toHaveBeenCalled();
        expect(disabledRun).not.toHaveBeenCalled();
    });

    it("tears down live contributions in reverse order and contains cleanup failures", () => {
        const registry = new ActionRegistry();
        const order: string[] = [];
        const failure = new Error("middle cleanup failed");
        const first = registry.register(action("first.action"), { onDispose: () => order.push("first") });
        const second = registry.register(action("second.action"), {
            onDispose: () => {
                order.push("second");
                throw failure;
            },
        });
        const third = registry.register(action("third.action"), { onDispose: () => order.push("third") });

        let thrown: unknown;
        try {
            registry.dispose();
        } catch (error) {
            thrown = error;
        }

        expect(order).toEqual(["third", "second", "first"]);
        expect(thrown).toBeInstanceOf(AggregateError);
        expect((thrown as AggregateError).errors).toEqual([failure]);
        expect(registry.size).toBe(0);
        expect(registry.isDisposed).toBe(true);
        expect(first.disposed && second.disposed && third.disposed).toBe(true);
        expect(() => registry.register(action("late.action"))).toThrow(ActionRegistryDisposedError);

        registry.dispose();
        expect(order).toEqual(["third", "second", "first"]);
    });
});
