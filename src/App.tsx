import { lazy, Suspense, useEffect, useRef, useState, useSyncExternalStore } from "react";
import { invokeCommand as invoke } from "./api/invoke";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { checkForUpdate } from "./api/updater";
import { TopBar } from "./components/TopBar";
import { SideRail } from "./components/SideRail";
import { AgentRail } from "./components/AgentRail";
import { SidebarPeek } from "./components/SidebarPeek";
import { AgentSessionSync } from "./components/AgentSessionSync";
import { AgentLifecycleManager } from "./components/AgentLifecycleManager";
import { AgentPalettePortal as AgentPalette } from "./components/AgentPalettePortal";
import { FilePalette } from "./components/FilePalette";
import { SeshPicker } from "./components/SeshPicker";
import { SessionSwitcher } from "./components/SessionSwitcher";
import { AwsAuthModal } from "./components/aws/AwsAuthModal";
import { RundeckJobPalette } from "./components/rundeck/RundeckJobPalette";
import { BrunoRequestPalette } from "./components/bruno/BrunoRequestPalette";
import { BrunoEnvPalette } from "./components/bruno/BrunoEnvPalette";
import { Workspace } from "./components/Workspace";
import { Toaster } from "./components/Toaster";
import { CommandPalette } from "./components/CommandPalette";
import { DiagnosticsOverlay, Onboarding, WhatsNewOverlay } from "./components/ExperienceOverlays";
import { DialogHost } from "./components/DialogHost";
import { TerminalPane } from "./terminal/TerminalPane";
import { CliOpenBridge } from "./components/CliOpenBridge";
import { git } from "./api/git";
import { runKeybindingAction, useKeymap } from "./keymap";
import { filesApi } from "./api/files";
import { emit, subscribe } from "./state/bus";
import * as cmd from "./state/commands";
import { applyHydrate, canFlushPersist, flushPersist, hydrationAllowsPersistence, subscribePersist, type HydrationResult } from "./state/persist";
import { dispatchFolder, dispatchPathDrop, resolvePathDropTarget } from "./state/dropRegistry";
import { notify, reportError, swallow } from "./state/toast";
import { confirmDialog } from "./state/dialog";
import { invalidate } from "./state/resources";
import { getState, useStore } from "./state/store";
import { applyTheme, applyWindowOpacity, registerCustomThemes } from "./themes/bus";
import { dirname } from "./lib/paths";
import type { StandaloneCommand } from "./commands/registry";
import type { ProjectConfigLoadResult } from "./projectConfig";
import { agentDetectionApi } from "./api/agentDetection";
import { projectActionCommand, trustProjectConfig } from "./projectConfigRuntime";
import { worktreeHasLiveOwners } from "./worktreeLifecycle";
import { performanceTelemetry } from "./lib/performance";
import { workbenchRuntime } from "./workbench/runtime";
import {
    activeProjectTaskInventoryMatches,
    appTaskRuntime,
    clearActiveProjectTasks,
    getAppTaskSnapshot,
    replaceActiveProjectTasks,
    subscribeAppTasks,
} from "./tasks/application";
import type { ResolvedTaskDefinition, TaskControllerSnapshot } from "./tasks/taskRegistry";
import {
    applicationActionContext,
    applicationActionContextFingerprint,
    executeApplicationAction,
    getApplicationActionRevision,
    loadApplicationActions,
    resolveApplicationActions,
    subscribeApplicationActions,
} from "./actions/bridge";
import { projectControllerBridge } from "./projects/controllerBridge";
import { getIpcTransport, type IpcUnsubscribe } from "./api/transport";

const SettingsPanel = lazy(() => import("./components/SettingsPanel").then((module) => ({ default: module.SettingsPanel })));

interface BootInfo {
    home: string;
    state: string;
    recent: string[];
}

export interface ActiveTaskControls {
    readonly canStop: boolean;
    readonly restartTaskId: string | null;
}

/**
 * Process ownership outlives task discovery. In particular, invalidating or
 * deleting sikemux.json must not hide the only control capable of stopping its
 * already-running PTY. Restart is stricter: it is available only while the
 * last task still resolves through the current trusted project inventory.
 */
export function activeTaskControls(snapshot: TaskControllerSnapshot | null, currentTasks: readonly ResolvedTaskDefinition[]): ActiveTaskControls {
    if (!snapshot || snapshot.disposed) return Object.freeze({ canStop: false, restartTaskId: null });
    const restartTaskId = snapshot.task && currentTasks.some((task) => task.id === snapshot.task!.id) ? snapshot.task.id : null;
    return Object.freeze({
        canStop: snapshot.activeRunId !== null,
        restartTaskId,
    });
}

export function subscribeGitChanged(signal?: AbortSignal): Promise<IpcUnsubscribe> {
    return getIpcTransport().subscribe<{ repo: string }>(
        "git_changed",
        (event) => {
            const repo = event.payload.repo || "";
            filesApi.invalidate(repo || undefined);
            invalidate((kind, args) => {
                if (!kind.startsWith("git.") && kind !== "files.list") return false;
                if (!repo) return true;
                return args[0] === repo;
            });
            emit({ type: "fs-changed", repo });
        },
        { signal },
    );
}

interface TreeDropTarget {
    rootPath: string;
    targetDir: string;
    highlightPath: string | null;
    dropEl: HTMLElement;
}

type ValidProjectConfig = Extract<ProjectConfigLoadResult, { readonly status: "valid" }>;

function activeProjectConfigMatches(project: string, expected: ValidProjectConfig, requireTaskInventory = false): boolean {
    const active = projectControllerBridge.getActiveSnapshot();
    const current = active?.config;
    return (
        active?.cwd === project &&
        current?.status === "valid" &&
        current.path === expected.path &&
        current.fingerprint === expected.fingerprint &&
        (!requireTaskInventory || activeProjectTaskInventoryMatches(project, expected.fingerprint))
    );
}

/**
 * Re-checks the on-disk config after approval: the user can take arbitrarily
 * long in the dialog, and the project must still be the one they approved.
 */
async function trustCurrentProjectConfig(project: string, expected: ValidProjectConfig, requireTaskInventory = false): Promise<boolean> {
    if (!activeProjectConfigMatches(project, expected, requireTaskInventory)) return false;
    if (!(await trustProjectConfig(expected))) return false;
    return activeProjectConfigMatches(project, expected, requireTaskInventory);
}

/** `if (!trusted) return;` in promise form, for command bodies that must stay void. */
function whenTrusted(project: string, expected: ValidProjectConfig, requireTaskInventory: boolean, run: () => void): void {
    void trustCurrentProjectConfig(project, expected, requireTaskInventory).then((ok) => {
        if (ok) run();
    });
}

function elementAtPhysicalPosition(pos: { x: number; y: number }): HTMLElement | null {
    const dpr = window.devicePixelRatio || 1;
    return document.elementFromPoint(pos.x / dpr, pos.y / dpr) as HTMLElement | null;
}

function folderDropElement(treeRoot: HTMLElement, rootPath: string, dir: string): HTMLElement | null {
    if (dir === rootPath) return treeRoot;
    for (const row of treeRoot.querySelectorAll<HTMLElement>(".tree-row.is-folder")) {
        if (row.dataset.folderPath === dir) return row;
    }
    return null;
}

function resolveTreeDropTarget(at: HTMLElement | null): TreeDropTarget | null {
    const treeRoot = at?.closest(".ed-tree-scroll") as HTMLElement | null;
    const rootPath = treeRoot?.dataset.rootPath;
    if (!treeRoot || !rootPath) return null;

    const folder = at?.closest(".tree-row.is-folder") as HTMLElement | null;
    if (folder && treeRoot.contains(folder) && folder.dataset.folderPath) {
        return {
            rootPath,
            targetDir: folder.dataset.folderPath,
            highlightPath: folder.dataset.folderPath,
            dropEl: folder,
        };
    }

    const file = at?.closest(".tree-row.file") as HTMLElement | null;
    if (file && treeRoot.contains(file) && file.dataset.filePath) {
        const targetDir = file.dataset.dropDir || dirname(file.dataset.filePath);
        const dropEl = folderDropElement(treeRoot, rootPath, targetDir);
        if (!dropEl) return null;
        return {
            rootPath,
            targetDir,
            highlightPath: targetDir === rootPath ? null : targetDir,
            dropEl,
        };
    }

    return {
        rootPath,
        targetDir: rootPath,
        highlightPath: null,
        dropEl: treeRoot,
    };
}

export default function App() {
    useKeymap();
    const [bootReady, setBootReady] = useState(false);
    const [bootIssue, setBootIssue] = useState<string | null>(null);
    const zen = useStore((s) => s.zenMode);
    const leftRailOpen = useStore((s) => s.leftRailOpen);
    const rightRailOpen = useStore((s) => s.rightRailOpen);
    const leftOpen = leftRailOpen && !zen;
    const rightOpen = rightRailOpen && !zen;
    const activeSessionIsProject = useStore((s) => s.sessions[s.activeSessionId]?.kind === "project");
    const pickerOpen = useStore((s) => s.pickerOpen);
    const agentPaletteOpen = useStore((s) => s.agentPaletteOpen);
    const filePaletteOpen = useStore((s) => s.filePaletteOpen);
    const rundeckJobPaletteOpen = useStore((s) => s.rundeckJobPaletteOpen);
    const brunoReqPaletteOpen = useStore((s) => s.brunoReqPaletteOpen);
    const brunoEnvPaletteOpen = useStore((s) => s.brunoEnvPaletteOpen);
    const settingsOpen = useStore((s) => s.settingsOpen);
    const commandPaletteOpen = useStore((s) => s.commandPaletteOpen);
    const commandPopup = useStore((s) => s.commandPopup);
    const keybindingOverrides = useStore((s) => s.keybindingOverrides);
    const customCommands = useStore((s) => s.customCommands);
    const recentCommandKeys = useStore((s) => s.recentCommandKeys);
    const activeKind = useStore((s) => s.sessions[s.activeSessionId]?.kind ?? null);
    const activeSessionId = useStore((s) => s.activeSessionId);
    const activeProjectCwd = useStore((s) => {
        const session = s.sessions[s.activeSessionId];
        return session?.kind === "project" ? session.cwd : "";
    });
    const projectControllerSnapshot = useSyncExternalStore(
        projectControllerBridge.subscribe,
        projectControllerBridge.getActiveSnapshot,
        projectControllerBridge.getActiveSnapshot,
    );
    const activeProjectSnapshot = projectControllerSnapshot?.cwd === activeProjectCwd ? projectControllerSnapshot : null;
    const projectConfig = activeProjectSnapshot?.config ?? null;
    const taskRegistrySnapshot = useSyncExternalStore(subscribeAppTasks, getAppTaskSnapshot, getAppTaskSnapshot);
    useSyncExternalStore(subscribeApplicationActions, getApplicationActionRevision, getApplicationActionRevision);
    useStore(applicationActionContextFingerprint);
    const actionContext = applicationActionContext(getState());
    const activeWorktrees = activeProjectSnapshot?.worktrees ?? [];
    const activeTerminalWindowId = useStore((s) => {
        const id = s.sessions[s.activeSessionId]?.activeWindowId;
        return id && s.windows[id]?.role === "term" ? id : null;
    });
    const awsAuthModal = useStore((s) => s.awsAuthModal);
    const sessionSwitcherOpen = useStore((s) => s.sessionSwitcher !== null);
    const projectRepoKey = useStore((s) =>
        s.sessionOrder
            .map((id) => {
                const sess = s.sessions[id];
                return sess?.kind === "project" ? sess.cwd : "";
            })
            .filter(Boolean)
            .join("\0"),
    );
    const taskProjectRootsRef = useRef<Set<string>>(new Set());
    const runStandalone =
        (id: string, execute: () => void): (() => void) =>
        () => {
            cmd.noteRecentCommand(`standalone:${id}`);
            execute();
        };
    const contributedActionCommands: StandaloneCommand[] = resolveApplicationActions(actionContext).map((action) => ({
        id: action.commandId,
        title: action.title,
        detail: action.detail,
        category: action.category,
        shortcut: action.shortcut,
        disabled: !action.enabled,
        execute: () => {
            void executeApplicationAction(action.actionId, applicationActionContext(getState())).catch(reportError(`run action ${action.commandId}`));
        },
    }));
    const projectCommands: StandaloneCommand[] = [];
    if (projectConfig?.status === "valid") {
        if (projectConfig.config.preview?.command) {
            projectCommands.push({
                id: "project.preview.start",
                title: "Start project preview",
                detail: projectConfig.config.preview.url ? `Serve ${projectConfig.config.preview.url}` : "Run the checked-in preview command",
                category: "Project · Preview",
                execute: runStandalone("project.preview.start", () => {
                    void trustProjectConfig(projectConfig).then((ok) => {
                        if (!ok) return;
                        cmd.runCustomCommand({
                            id: "project.preview.start",
                            title: "Project preview",
                            detail: "Checked-in preview command",
                            command: projectConfig.config.preview!.command!,
                            contexts: ["project"],
                            placement: "terminal",
                        });
                    });
                }),
            });
        }
        if (projectConfig.config.preview?.url) {
            projectCommands.push({
                id: "project.preview.open",
                title: "Open project preview",
                detail: projectConfig.config.preview.url,
                category: "Project · Preview",
                execute: runStandalone("project.preview.open", () => {
                    void invoke("open_url", { url: projectConfig.config.preview!.url!, app: null, shortcut: null }).catch(
                        reportError("open project preview"),
                    );
                }),
            });
        }
    } else if (projectConfig?.status === "invalid") {
        projectCommands.push({
            id: "project.config.invalid",
            title: "Project config needs attention",
            detail: projectConfig.errors[0]?.message ?? "sikemux.json is invalid",
            category: "Project · sikemux.json",
            execute: runStandalone("project.config.invalid", () =>
                notify("error", `sikemux.json: ${projectConfig.errors.map((error) => `${error.path} ${error.message}`).join(" · ")}`),
            ),
        });
    }
    const currentProjectTasks =
        projectConfig?.status === "valid" && activeProjectTaskInventoryMatches(activeProjectCwd, projectConfig.fingerprint)
            ? taskRegistrySnapshot.tasks.filter((task) => task.project === activeProjectCwd && task.source === "project")
            : [];
    const taskCommands: StandaloneCommand[] =
        projectConfig?.status === "valid"
            ? currentProjectTasks.map((task) => ({
                  id: `task.run.${task.id}`,
                  title: task.label,
                  detail: `Run task from ${task.cwd}`,
                  category: "Tasks",
                  execute: runStandalone(`task.run.${task.id}`, () => {
                      whenTrusted(activeProjectCwd, projectConfig, true, () => {
                          const snapshot = appTaskRuntime.getSnapshot(activeProjectCwd);
                          const operation =
                              snapshot?.status === "running" || snapshot?.status === "stopping"
                                  ? appTaskRuntime.restart(activeProjectCwd, task.id)
                                  : appTaskRuntime.run(activeProjectCwd, task.id);
                          void operation.then(() => notify("success", `Started task: ${task.label}`)).catch(reportError(`start task ${task.label}`));
                      });
                  }),
              }))
            : [];
    const taskRuntimeSnapshot = activeProjectCwd ? appTaskRuntime.getSnapshot(activeProjectCwd) : null;
    const taskControls = activeTaskControls(taskRuntimeSnapshot, currentProjectTasks);
    if (activeProjectCwd && taskControls.restartTaskId) {
        const restartTaskId = taskControls.restartTaskId;
        taskCommands.push({
            id: "task.restart-active",
            title: "Restart active task",
            detail: "Stop the current project task and start its current trusted definition",
            category: "Tasks",
            execute: runStandalone("task.restart-active", () => {
                if (projectConfig?.status !== "valid") return;
                whenTrusted(activeProjectCwd, projectConfig, true, () => {
                    void appTaskRuntime
                        .restart(activeProjectCwd, restartTaskId)
                        .then(() => notify("success", "Restarted active task"))
                        .catch(reportError("restart active task"));
                });
            }),
        });
    }
    if (activeProjectCwd && taskControls.canStop) {
        taskCommands.push({
            id: "task.stop-active",
            title: "Stop active task",
            detail: "Stop only the exact PTY owned by the current project task",
            category: "Tasks",
            execute: runStandalone("task.stop-active", () => {
                void appTaskRuntime
                    .stop(activeProjectCwd)
                    .then(() => notify("success", "Stopped active task"))
                    .catch(reportError("stop active task"));
            }),
        });
    }
    const worktreeCommands: StandaloneCommand[] = activeWorktrees
        .filter((worktree) => !worktree.bare)
        .flatMap((worktree) => [
            {
                id: `worktree.open.${worktree.path}`,
                title: worktree.current ? "Open current worktree" : `Open worktree: ${worktree.branch ?? "detached"}`,
                detail: worktree.path,
                category: "Project · Worktrees",
                execute: runStandalone(`worktree.open.${worktree.path}`, () => cmd.createProjectSession(worktree.path)),
            },
            ...(!worktree.is_main && !worktree.current
                ? [
                      {
                          id: `worktree.remove.${worktree.path}`,
                          title: `Remove worktree: ${worktree.branch ?? "detached"}`,
                          detail: "Safe removal; dirty worktrees are refused",
                          category: "Project · Worktrees",
                          execute: runStandalone(`worktree.remove.${worktree.path}`, () => {
                              if (worktreeHasLiveOwners(getState(), worktree.path)) {
                                  notify("info", "Close the worktree’s Sikemux project and agents before removing it");
                                  return;
                              }
                              void confirmDialog({
                                  title: `Remove worktree ${worktree.branch ?? worktree.path}?`,
                                  body: "Dirty worktrees will be refused.",
                                  confirmLabel: "Remove",
                                  destructive: true,
                              }).then((ok) => {
                                  if (!ok) return;
                                  return git
                                      .worktreeRemove(activeProjectCwd, worktree.path)
                                      .then(async () => {
                                          await projectControllerBridge.refresh(activeProjectCwd);
                                          notify("success", `Removed worktree ${worktree.branch ?? worktree.path}`);
                                      })
                                      .catch(reportError("remove worktree"));
                              });
                          }),
                      } satisfies StandaloneCommand,
                  ]
                : []),
        ]);
    const standaloneCommands: StandaloneCommand[] = [
        ...(activeKind === "project"
            ? [
                  {
                      id: "agents.launch",
                      title: "Open an agent CLI",
                      detail: "Choose a local provider and open it directly in a PTY",
                      category: "Agents",
                      execute: runStandalone("agents.launch", cmd.openAgentPalette),
                  } satisfies StandaloneCommand,
              ]
            : []),
        {
            id: "support.diagnostics",
            title: "Open runtime diagnostics",
            detail: "Inspect redacted runtime and agent-detection health",
            category: "Support",
            execute: runStandalone("support.diagnostics", cmd.openDiagnostics),
        },
        {
            id: "support.whats-new",
            title: "Open What’s New",
            detail: "Review the latest Sikemux release notes",
            category: "Support",
            execute: runStandalone("support.whats-new", cmd.openWhatsNew),
        },
        {
            id: "support.onboarding",
            title: "Replay onboarding",
            detail: "Open the first-run Sikemux walkthrough",
            category: "Support",
            execute: runStandalone("support.onboarding", cmd.openOnboarding),
        },
        {
            id: "session.export",
            title: "Copy active session bundle",
            detail: "Export a safe session copy to the clipboard",
            category: "Session",
            execute: runStandalone("session.export", () => void cmd.exportActiveSession().catch(reportError("session export"))),
        },
        {
            id: "session.import",
            title: "Import session from clipboard",
            detail: "Validate and import a safe dormant session copy",
            category: "Session",
            execute: runStandalone("session.import", () => void cmd.importSessionFromClipboard().catch(reportError("session import"))),
        },
        ...(activeTerminalWindowId
            ? [
                  {
                      id: "window.duplicate",
                      title: "Duplicate active terminal",
                      detail: "Clone the active window into a new terminal tab",
                      category: "Window",
                      execute: runStandalone("window.duplicate", () => cmd.duplicateWindow(activeTerminalWindowId)),
                  } satisfies StandaloneCommand,
              ]
            : []),
        {
            id: "agents.reload-manifests",
            title: "Reload agent manifests",
            detail: "Reload agent-state detection rules from disk",
            category: "Agents",
            execute: runStandalone("agents.reload-manifests", () => void agentDetectionApi.reload().catch(reportError("agent manifest reload"))),
        },
        ...worktreeCommands,
        ...taskCommands,
        ...contributedActionCommands,
        ...projectCommands,
    ];

    useEffect(() => {
        if (activeProjectCwd && projectConfig?.status === "valid") {
            replaceActiveProjectTasks(activeProjectCwd, projectConfig.fingerprint, projectConfig.config.tasks);
        } else {
            clearActiveProjectTasks();
        }
        return clearActiveProjectTasks;
    }, [activeProjectCwd, projectConfig]);

    useEffect(() => {
        let cancelled = false;
        let dispose: (() => void) | null = null;
        if (!activeProjectCwd || projectConfig?.status !== "valid") return;

        void loadApplicationActions()
            .then((runtime) => {
                if (cancelled) return;
                const registration = runtime.registerProjectActions({
                    projectId: activeSessionId,
                    projectRoot: activeProjectCwd,
                    configPath: projectConfig.path,
                    actions: projectConfig.config.actions,
                    isCurrent: () => activeProjectConfigMatches(activeProjectCwd, projectConfig),
                    execute: (action) => {
                        whenTrusted(activeProjectCwd, projectConfig, false, () => cmd.runCustomCommand(projectActionCommand(action)));
                    },
                });
                dispose = () => registration.dispose();
            })
            .catch((error: unknown) => {
                if (!cancelled) reportError("load project actions")(error);
            });

        return () => {
            cancelled = true;
            dispose?.();
        };
    }, [activeProjectCwd, activeSessionId, projectConfig]);

    useEffect(() => {
        const current = new Set(projectRepoKey.split("\0").filter(Boolean));
        for (const project of taskProjectRootsRef.current) {
            if (!current.has(project)) void appTaskRuntime.disposeProject(project).catch(reportError("stop closed-project task"));
        }
        taskProjectRootsRef.current = current;
    }, [projectRepoKey]);

    useEffect(() => {
        if (!bootReady) return;
        void projectControllerBridge.start();
        return () => {
            projectControllerBridge.stop();
        };
    }, [bootReady]);

    useEffect(() => {
        const roots = projectRepoKey ? projectRepoKey.split("\0") : [];
        void projectControllerBridge.reconcile(roots, activeProjectCwd || null);
    }, [activeProjectCwd, projectRepoKey]);

    useEffect(() => {
        let disposed = false;
        let unsub = () => {};
        let bootFinished = false;
        let hydrationResult: HydrationResult | null = null;
        const bootSpan = performanceTelemetry.startTrace("startup.boot");
        const finishBoot = (outcome: "success" | "error" | "cancelled") => {
            if (bootFinished) return;
            bootFinished = true;
            const recorded = performanceTelemetry.endSpan(bootSpan, { outcome });
            if (recorded) performanceTelemetry.recordLatency("startup.boot", recorded.durationMs);
        };
        invoke<BootInfo>("boot_init")
            .then((boot) => {
                if (disposed) return;
                const hydrateSpan = performanceTelemetry.startSpan(bootSpan, "startup.hydrate");
                try {
                    cmd.setHome(boot.home);
                    hydrationResult = applyHydrate(boot.state);
                    const st = getState();
                    registerCustomThemes(st.customThemes);
                    applyTheme(st.themeId);
                    applyWindowOpacity(st.windowOpacity);
                    if (st.themeMode === "system") cmd.applySystemTheme(window.matchMedia("(prefers-color-scheme: dark)").matches);
                    cmd.setWindowBlur(st.windowBlur);
                    if (hydrationAllowsPersistence(hydrationResult)) {
                        if (!st.onboardingComplete) cmd.openOnboarding();
                        else if (st.lastReleaseNotes && st.lastSeenVersion !== st.lastReleaseNotes.version) cmd.openWhatsNew();
                        performanceTelemetry.endSpan(hydrateSpan, { outcome: "success" });
                    } else {
                        setBootIssue(
                            hydrationResult === "unsupported-future"
                                ? "This workspace was saved by a newer Sikemux version. It has not been opened or modified. Update Sikemux, then reload."
                                : "Saved workspace state could not be validated. Persistence is disabled so the original data is not overwritten.",
                        );
                        performanceTelemetry.endSpan(hydrateSpan, { outcome: "error" });
                    }
                } catch (error) {
                    performanceTelemetry.endSpan(hydrateSpan, { outcome: "error" });
                    throw error;
                }
            })
            .catch((error) => {
                if (!disposed) setBootIssue("Sikemux could not load workspace state. Nothing has been written; reload to retry.");
                finishBoot("error");
                swallow("boot_init")(error);
            })
            .finally(() => {
                const writable = hydrationResult !== null && hydrationAllowsPersistence(hydrationResult);
                if (!disposed && writable) {
                    workbenchRuntime.start();
                    unsub = subscribePersist();
                    setBootReady(true);
                }
                finishBoot(disposed ? "cancelled" : writable ? "success" : "error");
            });
        return () => {
            disposed = true;
            finishBoot("cancelled");
            unsub();
            workbenchRuntime.stop();
        };
    }, []);

    useEffect(
        () =>
            useStore.subscribe((state, previous) => {
                if (state.activeSessionId !== previous.activeSessionId && previous.sessions[previous.activeSessionId]) {
                    cmd.setLastSessionId(previous.activeSessionId);
                }
            }),
        [],
    );

    useEffect(() => {
        const media = window.matchMedia("(prefers-color-scheme: dark)");
        const apply = () => cmd.applySystemTheme(media.matches);
        media.addEventListener("change", apply);
        return () => media.removeEventListener("change", apply);
    }, []);

    useEffect(() => {
        let disposed = false;
        let closing = false;
        const onPageHide = () => {
            if (canFlushPersist()) void flushPersist();
        };
        window.addEventListener("pagehide", onPageHide);
        const closeListener = getCurrentWindow()
            .onCloseRequested(async (event) => {
                if (closing) return;
                // boot_init may still be loading the durable snapshot. Let Tauri
                // close normally instead of replacing it with initial UI state.
                if (!canFlushPersist()) return;
                closing = true;
                event.preventDefault();
                try {
                    const saved = (await flushPersist()) || (await flushPersist());
                    if (saved && !disposed) await getCurrentWindow().destroy();
                } finally {
                    closing = false;
                }
            })
            .catch(swallow("close persistence"));
        return () => {
            disposed = true;
            window.removeEventListener("pagehide", onPageHide);
            void closeListener.then((unlisten) => {
                if (typeof unlisten === "function") unlisten();
            });
        };
    }, []);

    useEffect(() => {
        const controller = new AbortController();
        void subscribeGitChanged(controller.signal).catch((error: unknown) => {
            if (!controller.signal.aborted) swallow("git change listener")(error);
        });
        return () => {
            controller.abort();
        };
    }, []);

    useEffect(() => {
        return subscribe("rnd-auth-expired", () => {
            invalidate((kind) => kind.startsWith("rnd."));
        });
    }, []);

    useEffect(() => {
        return subscribe("aws-auth-expired", () => {
            invalidate((kind) => kind.startsWith("aws."));
        });
    }, []);

    useEffect(() => {
        const firstCheck = window.setTimeout(() => void checkForUpdate(), 4000);
        const poll = window.setInterval(() => void checkForUpdate(), 30 * 60_000);
        return () => {
            window.clearTimeout(firstCheck);
            window.clearInterval(poll);
        };
    }, []);

    useEffect(() => {
        let hoveredPathTarget: HTMLElement | null = null;

        const clearTreeHover = () => {
            emit({ type: "tree-native-drag-hover", cwd: null, targetDir: null, highlightPath: null });
        };

        const setPathHover = (target: HTMLElement | null) => {
            if (hoveredPathTarget === target) return;
            if (hoveredPathTarget) delete hoveredPathTarget.dataset.nativePathDragOver;
            hoveredPathTarget = target;
            if (hoveredPathTarget) hoveredPathTarget.dataset.nativePathDragOver = "true";
        };

        const emitTreeHover = (at: HTMLElement | null) => {
            const target = resolveTreeDropTarget(at);
            emit({
                type: "tree-native-drag-hover",
                cwd: target?.rootPath ?? null,
                targetDir: target?.targetDir ?? null,
                highlightPath: target?.highlightPath ?? null,
            });
        };

        const unlistenP = getCurrentWebview().onDragDropEvent((e) => {
            if (e.payload.type === "leave") {
                setPathHover(null);
                clearTreeHover();
                return;
            }

            const at = elementAtPhysicalPosition(e.payload.position);

            if (e.payload.type === "enter" || e.payload.type === "over") {
                const pathTarget = resolvePathDropTarget(at);
                setPathHover(pathTarget);
                if (pathTarget) clearTreeHover();
                else emitTreeHover(at);
                return;
            }

            setPathHover(null);
            const paths = e.payload.paths;
            if (!paths || paths.length === 0) {
                clearTreeHover();
                return;
            }
            if (dispatchPathDrop(at, paths)) {
                clearTreeHover();
                return;
            }
            const target = resolveTreeDropTarget(at);
            if (target) dispatchFolder(target.dropEl, paths);
            clearTreeHover();
        });
        return () => {
            setPathHover(null);
            void unlistenP.then((u) => u());
        };
    }, []);

    if (!bootReady) {
        return (
            <div className="shell boot-shell">
                <section className="boot-state" role={bootIssue ? "alert" : "status"} aria-live="polite">
                    <span className="boot-state-mark" aria-hidden="true">
                        S
                    </span>
                    <h1>{bootIssue ? "Workspace protected" : "Opening workspace"}</h1>
                    <p>{bootIssue ?? "Hydrating sessions and preparing terminal ownership…"}</p>
                    {bootIssue && (
                        <button type="button" onClick={() => window.location.reload()}>
                            Reload safely
                        </button>
                    )}
                </section>
                <Toaster />
            </div>
        );
    }

    return (
        <div className="shell">
            <CliOpenBridge />
            <AgentSessionSync />
            <AgentLifecycleManager />
            <TopBar />
            <div className="body">
                {leftOpen && <SideRail />}
                {!leftRailOpen && !zen && (
                    <SidebarPeek side="left">
                        <SideRail />
                    </SidebarPeek>
                )}
                <main className={`stage${settingsOpen ? " stage--settings" : ""}`}>
                    <Workspace />
                    {settingsOpen && (
                        <Suspense fallback={null}>
                            <SettingsPanel />
                        </Suspense>
                    )}
                </main>
                {rightOpen && activeSessionIsProject && <AgentRail />}
                {!rightRailOpen && !zen && activeSessionIsProject && (
                    <SidebarPeek side="right">
                        <AgentRail />
                    </SidebarPeek>
                )}
            </div>
            {pickerOpen && <SeshPicker />}
            {agentPaletteOpen && <AgentPalette />}
            {filePaletteOpen && <FilePalette />}
            {rundeckJobPaletteOpen && <RundeckJobPalette />}
            {brunoReqPaletteOpen && <BrunoRequestPalette />}
            {brunoEnvPaletteOpen && <BrunoEnvPalette />}
            {awsAuthModal && <AwsAuthModal />}
            {sessionSwitcherOpen && <SessionSwitcher />}
            {commandPaletteOpen && (
                <CommandPalette
                    keybindingOverrides={keybindingOverrides}
                    customCommands={customCommands}
                    recentCommandKeys={recentCommandKeys}
                    standaloneCommands={standaloneCommands}
                    context={activeKind}
                    onClose={cmd.closeCommandPalette}
                    onExecute={cmd.noteRecentCommand}
                    executeBuiltin={(id) => {
                        runKeybindingAction(id, new KeyboardEvent("keydown"), getState());
                    }}
                    executeCustom={(command) => {
                        cmd.runCustomCommand(command);
                    }}
                />
            )}
            {commandPopup && (
                <div className="experience-backdrop command-popup-backdrop" role="presentation" onMouseDown={cmd.closeCommandPopup}>
                    <section
                        className="command-popup"
                        role="dialog"
                        aria-modal="true"
                        aria-label={commandPopup.title}
                        onMouseDown={(event) => event.stopPropagation()}>
                        <header>
                            <span>{commandPopup.title}</span>
                            <button onClick={cmd.closeCommandPopup}>close</button>
                        </header>
                        <TerminalPane
                            key={commandPopup.id}
                            cwd={commandPopup.cwd || undefined}
                            startup={commandPopup.startup}
                            context={commandPopup.context}
                            active
                            visible
                        />
                    </section>
                </div>
            )}
            <Onboarding />
            <DiagnosticsOverlay />
            <WhatsNewOverlay />
            <DialogHost />
            <Toaster />
        </div>
    );
}
