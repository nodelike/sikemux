import { describe, expect, it, vi } from "vitest";
import { keybindingLabel } from "../commands/keybindings";
import type { ProjectAction } from "../projects/projectConfig";
import { ActionNotVisibleError, type ActionContextInput } from "./registry";
import { ApplicationActionRuntime, StaleProjectActionConfigurationError } from "./application";

const projectContext: ActionContextInput = {
    focusedItem: { id: "pane-1", kind: "terminal" },
    session: { id: "session-1", kind: "project" },
    project: { id: "session-1", root: "/workspace/project" },
    focus: { target: "workbench-item", editable: false },
    capabilities: ["session.project", "item.terminal"],
};

describe("ApplicationActionRuntime", () => {
    it("adapts contextual trusted contributions and revokes them atomically", async () => {
        const runtime = new ApplicationActionRuntime();
        const run = vi.fn(() => "done");
        const changed = vi.fn();
        runtime.subscribe(changed);
        const registration = runtime.register({
            id: "sikemux.test-actions",
            actions: [
                {
                    id: "inspect",
                    create: () => ({
                        commandId: "test.inspect",
                        scope: { kind: "project", id: "session-1" },
                        definition: {
                            id: "test.inspect",
                            title: "Inspect project",
                            detail: "Inspect the active project",
                            category: "Test",
                            source: "test.actions",
                            defaultBinding: "Meta+Shift+KeyI",
                            run,
                        },
                    }),
                },
                {
                    id: "hidden",
                    create: () => ({
                        commandId: "test.hidden",
                        definition: {
                            id: "test.hidden",
                            title: "Hidden action",
                            detail: "Hidden in this context",
                            category: "Test",
                            source: "test.actions",
                            defaultBinding: null,
                            when: () => false,
                            run: () => undefined,
                        },
                    }),
                },
            ],
        });

        expect(registration.activeContributions).toBe(2);
        expect(changed).toHaveBeenCalledOnce();
        expect(runtime.resolve(projectContext)).toEqual([
            expect.objectContaining({
                actionId: "test.inspect",
                commandId: "test.inspect",
                shortcut: keybindingLabel("Meta+Shift+KeyI"),
                enabled: true,
            }),
        ]);
        expect(runtime.resolve({ ...projectContext, project: { id: "other", root: "/workspace/other" } })).toEqual([]);
        expect(runtime.matchKeybinding({ code: "KeyI", metaKey: true, ctrlKey: false, altKey: false, shiftKey: true }, projectContext)).toEqual({
            actionId: "test.inspect",
            commandId: "test.inspect",
        });
        await expect(runtime.execute("test.inspect", projectContext)).resolves.toBe("done");
        expect(run).toHaveBeenCalledOnce();

        registration.dispose();
        registration.dispose();
        expect(runtime.resolve(projectContext)).toEqual([]);
        expect(runtime.getHostSnapshot()).toMatchObject({ extensionCount: 0, activeContributions: 0 });
        expect(changed).toHaveBeenCalledTimes(2);
        runtime.dispose();
    });

    it("preserves project command IDs and owns configured keybindings", async () => {
        const runtime = new ApplicationActionRuntime();
        const execute = vi.fn();
        const action: ProjectAction = {
            id: "Quality.Check",
            label: "Run quality check",
            description: "Lint and test the project",
            command: "pnpm check",
            placement: "terminal",
            contexts: ["project"],
            keybinding: "Meta+Shift+KeyT",
        };
        const registration = runtime.registerProjectActions({
            projectId: "session-1",
            projectRoot: "/workspace/project",
            configPath: "/workspace/project/sikemux.json",
            actions: [action],
            isCurrent: () => true,
            execute,
        });
        const otherExecute = vi.fn();
        const otherRegistration = runtime.registerProjectActions({
            projectId: "session-2",
            projectRoot: "/workspace/other",
            configPath: "/workspace/other/sikemux.json",
            actions: [{ ...action, id: "constructor" }, action],
            isCurrent: () => true,
            execute: otherExecute,
        });

        expect(registration.activeContributions).toBe(1);
        expect(otherRegistration.activeContributions).toBe(2);
        const extensionIds = runtime.getHostSnapshot().extensions.map(({ id }) => id);
        expect(extensionIds).toHaveLength(2);
        expect(new Set(extensionIds).size).toBe(2);
        const resolved = runtime.resolve(projectContext);
        expect(resolved).toHaveLength(1);
        expect(resolved[0]).toMatchObject({
            actionId: expect.stringMatching(/^projectConfig\.action\.p/u),
            commandId: "project.action.Quality.Check",
            binding: "Meta+Shift+KeyT",
            shortcut: keybindingLabel("Meta+Shift+KeyT"),
            source: "project.config",
        });
        expect(runtime.resolve({ ...projectContext, project: { id: "session-1", root: "/workspace/replaced" } })).toEqual([]);

        const match = runtime.matchKeybinding({ code: "KeyT", metaKey: true, ctrlKey: false, altKey: false, shiftKey: true }, projectContext);
        expect(match).toEqual({ actionId: resolved[0].actionId, commandId: "project.action.Quality.Check" });
        await runtime.execute(match!.actionId, projectContext);
        expect(execute).toHaveBeenCalledWith(expect.objectContaining({ id: "Quality.Check", keybinding: "Meta+Shift+KeyT" }));

        const otherContext: ActionContextInput = {
            ...projectContext,
            session: { id: "session-2", kind: "project" },
            project: { id: "session-2", root: "/workspace/other" },
        };
        const otherResolved = runtime.resolve(otherContext);
        expect(otherResolved).toEqual(
            expect.arrayContaining([
                expect.objectContaining({ commandId: "project.action.constructor" }),
                expect.objectContaining({ commandId: "project.action.Quality.Check" }),
            ]),
        );
        const constructorAction = otherResolved.find(({ commandId }) => commandId === "project.action.constructor");
        await runtime.execute(constructorAction!.actionId, otherContext);
        expect(otherExecute).toHaveBeenCalledWith(expect.objectContaining({ id: "constructor" }));

        registration.dispose();
        expect(runtime.matchKeybinding({ code: "KeyT", metaKey: true, ctrlKey: false, altKey: false, shiftKey: true }, projectContext)).toBeNull();
        expect(runtime.resolve(otherContext)).toHaveLength(2);
        otherRegistration.dispose();
        runtime.dispose();
    });

    it("hides and rejects actions from a same-root config replaced before effect cleanup", async () => {
        const runtime = new ApplicationActionRuntime();
        const execute = vi.fn();
        let activeFingerprint = "config-a";
        let singleUseGuard = false;
        let guardChecks = 0;
        const registration = runtime.registerProjectActions({
            projectId: "session-1",
            projectRoot: "/workspace/project",
            configPath: "/workspace/project/sikemux.json",
            actions: [
                {
                    id: "deploy",
                    label: "Deploy old config",
                    description: "Must not survive a config replacement",
                    command: "deploy-a",
                    placement: "terminal",
                    contexts: ["project"],
                },
            ],
            isCurrent: () => {
                if (singleUseGuard) return guardChecks++ === 0;
                return activeFingerprint === "config-a";
            },
            execute,
        });
        const [action] = runtime.resolve(projectContext);

        expect(action).toBeDefined();
        activeFingerprint = "config-b";
        expect(runtime.resolve(projectContext)).toEqual([]);
        await expect(runtime.execute(action!.actionId, projectContext)).rejects.toBeInstanceOf(ActionNotVisibleError);
        expect(execute).not.toHaveBeenCalled();

        // Even if the current predicate changes between execution-time
        // resolution and run, the command itself revalidates once more.
        singleUseGuard = true;
        guardChecks = 0;
        await expect(runtime.execute(action!.actionId, projectContext)).rejects.toBeInstanceOf(StaleProjectActionConfigurationError);
        expect(execute).not.toHaveBeenCalled();

        registration.dispose();
        runtime.dispose();
    });

    it("contains contribution collisions while preserving execution errors", async () => {
        const runtime = new ApplicationActionRuntime();
        const failure = new Error("action failed");
        const first = runtime.register({
            id: "sikemux.first-actions",
            actions: [
                {
                    id: "shared",
                    create: () => ({
                        commandId: "shared.command",
                        definition: {
                            id: "first.action",
                            title: "First",
                            detail: "First action",
                            category: "Test",
                            source: "first.actions",
                            defaultBinding: null,
                            run: () => {
                                throw failure;
                            },
                        },
                    }),
                },
            ],
        });
        const collided = runtime.register({
            id: "sikemux.second-actions",
            actions: [
                {
                    id: "shared",
                    create: () => ({
                        commandId: "shared.command",
                        definition: {
                            id: "second.action",
                            title: "Second",
                            detail: "Second action",
                            category: "Test",
                            source: "second.actions",
                            defaultBinding: null,
                            run: () => undefined,
                        },
                    }),
                },
            ],
        });

        expect(collided.activeContributions).toBe(0);
        expect(runtime.getHostSnapshot()).toMatchObject({
            activeContributions: 1,
            failureCount: 1,
            failures: [expect.objectContaining({ stage: "register", contributionId: expect.stringContaining("second-actions") })],
        });
        await expect(runtime.execute("first.action", {})).rejects.toBe(failure);

        collided.dispose();
        first.dispose();
        runtime.dispose();
    });
});
