import "./styles/restored-rails.css";
import { lazy, Suspense, useEffect, useState, useSyncExternalStore } from "react";
import { invokeCommand as invoke } from "./api/invoke";
import { whenIdle } from "./lib/idle";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { followUpdates, UPDATES_IN_APP } from "./api/updater";
import { TabRow, TopBar } from "./shell/TopBar";
import { SideRail } from "./rail/SideRail";
import { TabRail } from "./rail/TabRail";
import { RailPeek } from "./rail/RailPeek";
import { RailResizer, useRailWidthVars } from "./rail/RailResizer";
import { watchSshLinks } from "./terminal/sshLinks";
import { watchTerminalTitles } from "./terminal/terminalTitles";
import { watchRailNames } from "./terminal/railNames";
import { watchFaceStyle } from "./buddies/faceStyle";
import { customCommandsOf, showPreferences, usePreference, watchPreferences, preferencesOf } from "./state/preferences";
import { watchDrafts } from "./state/gitWorkbench";
import { watchLspClaims } from "./api/lsp";
import { watchAgentStatus } from "./agents/statusRefresh";
import { syncAgentAccounts } from "./agents/accountBook";
import { agentApi, type AgentAccountBook } from "./api/agents";
import { AgentPalettePortal as AgentPalette } from "./agents/AgentPalettePortal";
import { AgentInstallHost } from "./agents/AgentInstall";
import { FilePalette } from "./palettes/FilePalette";
import { NewTabPalette } from "./palettes/NewTabPalette";
import { SeshPicker } from "./palettes/SeshPicker";
import { SessionSwitcher } from "./palettes/SessionSwitcher";
import { Workspace } from "./workspace/Workspace";
import { Toaster } from "./shell/Toaster";
import { CommandPalette } from "./palettes/CommandPalette";
import { ThemePalette } from "./palettes/ThemePalette";
import { AccountDialogHost } from "./agents/AccountDialog";
import { DialogHost } from "./shell/DialogHost";
import { ShareHost } from "./share/ShareControls";
import { BuddyPickerHost } from "./buddies/BuddyPicker";
import { ImageViewer } from "./editor/ImageViewer";
import { useOccludeNativeViews } from "./state/nativeViews";
import { TerminalPane } from "./terminal/TerminalPane";
import { HarnessBridge } from "./shell/HarnessBridge";
import { NetworkBridge } from "./shell/NetworkBridge";
import { GlobalSkillsBridge } from "./shell/GlobalSkillsBridge";
import { RemoteChatBridge } from "./shell/RemoteChatBridge";
import { AgentNameBridge } from "./shell/AgentNameBridge";
import { AgentQuestionBridge } from "./shell/AgentQuestionBridge";
import { AgentBoardBridge } from "./shell/AgentBoardBridge";
import { RemoteWorkspaceBridge } from "./shell/RemoteWorkspaceBridge";
import { UpdateRequiredPrompt } from "./shell/UpdateRequiredPrompt";
import { PairingPrompt } from "./shell/PairingPrompt";
import { CliOpenBridge } from "./shell/CliOpenBridge";
import { DeepLinkBridge } from "./shell/DeepLinkBridge";
import { git } from "./api/git";
import { runKeybindingAction, useKeymap } from "./commands/keymap";
import { getKeybindingAction, QUIT_AND_STOP_SHORTCUT, keybindingOverridesOf } from "./commands/keybindings";
import { usePinchZoom } from "./shell/pinchZoom";
import { introduceNotifications, useAgentNotifications } from "./agents/agentNotifications";
import { useVoiceDictation } from "./voice/dictation";
import { VoiceCaption } from "./voice/VoiceCaption";
import { useBackdropImage } from "./hooks/useBackdropImage";
import { useBrowserDownloads } from "./state/browserDownloads";
import { useBrowserReveal } from "./state/browserReveal";
import { useSimulatorReveal } from "./state/simulatorReveal";
import { simApi } from "./api/sim";
import { useBrowserStrips } from "./state/browserStrips";
import { filesApi } from "./api/files";
import { fsapi } from "./api/fs";
import { emit } from "./state/bus";
import * as cmd from "./state/commands";
import { offerSavedSessions, restoreCoreSessions } from "./workspace/restoreCoreSessions";
import { coreSessionsApi } from "./api/coreSessions";
import { coreCall } from "./api/core";
import type { Preferences, Workspace as SharedWorkspace } from "./api/generated/wire";
import { fetchCoreWorkspace, startCoreWorkspace } from "./state/coreWorkspace";
import { startHosts } from "./hosts/hosts";
import { applyHydrate, canFlushPersist, flushPersist, hydrationAllowsPersistence, subscribePersist, type HydrationResult } from "./state/persist";
import {
    dispatchFolder,
    dispatchPaths,
    focusedPathDropTarget,
    nativeDropPoint,
    pathDropTargetAt,
    resolvePathDropTarget,
    showPathDropHover,
} from "./state/dropRegistry";
import { notify, reportError, swallow } from "./state/toast";
import { whatsNew } from "./api/releases";
import { confirmDialog } from "./state/dialog";
import { invalidate } from "./state/resources";
import { getState, useStore } from "./state/store";
import type { Palette } from "./themes";
import { applyWindowOpacity, watchTheme, wearPicked } from "./themes/bus";
import { startPageTheme } from "./chat/pageTheme";
import { applyTerminalFontSize, currentTerminalFontSize } from "./terminal/fontSize";
import { applyChatTextScale } from "./chat/textScale";
import { applyEditorTextScale } from "./editor/textScale";
import { dirname } from "./lib/paths";
import type { StandaloneCommand } from "./commands/registry";
import { agentDetectionApi } from "./api/agentDetection";
import { lsp } from "./api/lsp";
import { trustProjectConfig } from "./projects/projectConfigRuntime";
import { projectActionCommands } from "./projects/projectActions";
import { worktreeHasLiveOwners } from "./git/worktreeLifecycle";
import { performanceTelemetry } from "./lib/performance";
import { releaseClosedPaneResources } from "./terminal/paneResources";
import { personRuns, runProjectTask, stopTaskRun } from "./tasks/projectTaskActions";
import { useProjectTasks, useTaskRuns, type ProjectTask, type TaskRun } from "./tasks/coreTasks";
import { showTaskTerminals } from "./tasks/taskTerminals";
import { activeProject, refreshProject, setOpenProjects, subscribeProjects, watchProjects } from "./projects/projectState";
import { onCoreChanged, type CoreChange } from "./api/coreChanged";
import { pluginsApi } from "./api/plugins";
import "./plugins/builtin";
import { watchInbox } from "./rail/useInbox";
import { useInstalledPlugins } from "./plugins/installed";
import { useRailDock } from "./rail/railMotion";
import { openUrl } from "./api/system";
import brandMark from "../brand/mark/mark-white.svg";

const SettingsPanel = lazy(() => import("./settings/SettingsPanel").then((module) => ({ default: module.SettingsPanel })));

/* The welcome, the release notes and the diagnostics panel, none of which exist
   until someone opens one. */
const Onboarding = lazy(() => import("./shell/ExperienceOverlays").then((module) => ({ default: module.Onboarding })));
const DiagnosticsOverlay = lazy(() => import("./shell/ExperienceOverlays").then((module) => ({ default: module.DiagnosticsOverlay })));
const WhatsNewOverlay = lazy(() => import("./shell/WhatsNewOverlay").then((module) => ({ default: module.WhatsNewOverlay })));

interface BootInfo {
    home: string;
    state: string;
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
export function activeTaskControls(running: TaskRun | null, currentTasks: readonly ProjectTask[]): ActiveTaskControls {
    if (!running) return Object.freeze({ canStop: false, restartTaskId: null });
    const restartTaskId = currentTasks.some((task) => task.id === running.taskId) ? running.taskId : null;
    return Object.freeze({ canStop: true, restartTaskId });
}

/** The part of `path` below `folder`, with forward slashes, or null when it is not below it. */
function below(folder: string, path: unknown): string | null {
    if (typeof path !== "string" || !(path.startsWith(`${folder}/`) || path.startsWith(`${folder}\\`))) return null;
    return path.slice(folder.length + 1).replaceAll("\\", "/");
}

export function gitChangeInvalidates(kind: string, args: unknown[], repo: string, paths: readonly string[] | null): boolean {
    // A project folder that holds repositories is watched as one tree, so a
    // change inside one of them arrives for the folder, not for that repository.
    const nested = repo ? below(repo, args[0]) : null;
    if (nested !== null) {
        const touched = !paths?.length || paths.some((path) => path === nested || path.startsWith(`${nested}/`));
        if (!touched || !kind.startsWith("git.")) return false;
        return !paths?.length || kind === "git.overview" || kind === "git.status" || kind === "git.changeOwners";
    }
    if (repo && args[0] !== repo) return false;
    if (kind === "files.list") return true;
    if (!kind.startsWith("git.")) return false;
    if (!paths?.length) return true;
    return kind === "git.overview" || kind === "git.status" || kind === "git.discoveredRepos" || kind === "git.changeOwners";
}

/** Reads again what the core says moved in a project: the paths, with `.git` standing for its git state, which anything may depend on. */
export function onGitChanged({ project, paths, host }: CoreChange): void {
    const repo = project ?? "";
    // An empty list would mean "these zero paths changed", and the scoped
    // consumers would refresh nothing, so it means "unknown" as none does.
    const moved = paths?.length && !paths.includes(".git") ? [...paths] : null;
    if (host === undefined) {
        filesApi.invalidate(repo || undefined);
        invalidate((kind, args) => gitChangeInvalidates(kind, args, repo, moved));
    }
    emit({ type: "fs-changed", repo, ...(moved ? { paths: moved } : {}), ...(host === undefined ? {} : { host }) });
}

interface TreeDropTarget {
    rootPath: string;
    targetDir: string;
    highlightPath: string | null;
    dropEl: HTMLElement;
}

/**
 * Which surface takes the paths a drag is carrying. The node under the cursor
 * answers first, then the file tree, which handles a drop of its own; a drop
 * that landed on neither still belongs somewhere, so the pane the point sits
 * inside takes it, and failing that the session last typed in.
 */
function pathDropSurface(at: HTMLElement | null, point: { x: number; y: number }): HTMLElement | null {
    const hit = resolvePathDropTarget(at);
    if (hit) return hit;
    if (resolveTreeDropTarget(at)) return null;
    return pathDropTargetAt(point) ?? focusedPathDropTarget();
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

function activeProjectCwdOf(s: ReturnType<typeof getState>): string {
    const session = s.sessions[s.activeSessionId];
    return session?.kind === "project" ? session.cwd : "";
}

function projectRepoKeyOf(s: ReturnType<typeof getState>): string {
    return s.sessionOrder
        .map((id) => {
            const session = s.sessions[id];
            return session?.kind === "project" ? session.cwd : "";
        })
        .filter(Boolean)
        .join("\0");
}

/*
 * The live project, kept in step with the controller and the task registry.
 *
 * All of it hangs off which project is in front, so it sits below the shell rather than inside it: the shell no longer re-renders
 * because a task started or a worktree list came back.
 */
function ProjectBridge() {
    const activeProjectCwd = useStore(activeProjectCwdOf);
    const projectRepoKey = useStore(projectRepoKeyOf);

    useEffect(() => watchProjects(), []);

    useEffect(() => {
        showTaskTerminals();
        return onCoreChanged("tasks", (change) => {
            if (change.project) void refreshProject(change.project);
        });
    }, []);

    useEffect(() => {
        const roots = projectRepoKey ? projectRepoKey.split("\0") : [];
        setOpenProjects(roots, activeProjectCwd || null);
    }, [activeProjectCwd, projectRepoKey]);

    return null;
}

/*
 * Everything the command palette can run, assembled only while it is open.
 *
 * This used to live in the shell's own render body: two hundred lines of
 * closures, rebuilt on every keystroke, every agent activity flip and every
 * command executed, for a list nobody was looking at. Mounting it with the
 * palette means the shell above stops subscribing to any of its inputs.
 */
async function quitAndStopEverything(): Promise<void> {
    if (canFlushPersist()) await flushPersist().catch(() => false);
    await coreSessionsApi.quitAndStopEverything().catch(reportError("quit and stop everything"));
}

function ApplicationCommandPalette() {
    const keybindingOverrides = useStore(keybindingOverridesOf);
    const customCommands = useStore(customCommandsOf);
    const recentCommandKeys = useStore((s) => s.recentCommandKeys);
    const activeKind = useStore((s) => s.sessions[s.activeSessionId]?.kind ?? null);
    const activeProjectCwd = useStore(activeProjectCwdOf);
    const languageServersAllowedHere = useStore((s) => (activeProjectCwd ? s.preferences?.languageServerTrust[activeProjectCwd] === true : false));
    const activeTerminalWindowId = useStore((s) => {
        const id = s.sessions[s.activeSessionId]?.activeWindowId;
        return id && s.windows[id]?.role === "term" ? id : null;
    });
    const projectControllerSnapshot = useSyncExternalStore(subscribeProjects, activeProject, activeProject);
    const activeProjectSnapshot = projectControllerSnapshot?.cwd === activeProjectCwd ? projectControllerSnapshot : null;
    const projectConfig = activeProjectSnapshot?.config ?? null;
    const activeWorktrees = activeProjectSnapshot?.worktrees ?? [];
    const offeredTasks = useProjectTasks(activeProjectCwd || null);
    const taskRuns = useTaskRuns();

    const runStandalone =
        (id: string, execute: () => void): (() => void) =>
        () => {
            cmd.noteRecentCommand(`standalone:${id}`);
            execute();
        };
    const projectCommands: StandaloneCommand[] = [];
    if (projectConfig?.status === "valid") {
        projectCommands.push(...projectActionCommands(activeProjectCwd, projectConfig));
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
                    void openUrl(projectConfig.config.preview!.url!).catch(reportError("open project preview"));
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
    const currentProjectTasks = offeredTasks?.tasks ?? [];
    const taskCommands: StandaloneCommand[] = currentProjectTasks.map((task) => ({
        id: `task.run.${task.id}`,
        title: task.label,
        detail: `Run task from ${task.cwd}`,
        category: "Tasks",
        execute: runStandalone(`task.run.${task.id}`, () => void runProjectTask(activeProjectCwd, task)),
    }));
    const runningTask = activeProjectCwd ? personRuns(taskRuns, activeProjectCwd).running : null;
    const taskControls = activeTaskControls(runningTask, currentProjectTasks);
    const restartTask = currentProjectTasks.find((task) => task.id === taskControls.restartTaskId);
    if (activeProjectCwd && restartTask) {
        taskCommands.push({
            id: "task.restart-active",
            title: "Restart active task",
            detail: "Stop the current project task and start its current trusted definition",
            category: "Tasks",
            execute: runStandalone("task.restart-active", () => void runProjectTask(activeProjectCwd, restartTask)),
        });
    }
    if (activeProjectCwd && runningTask && taskControls.canStop) {
        taskCommands.push({
            id: "task.stop-active",
            title: "Stop active task",
            detail: "Stop only the exact PTY owned by the current project task",
            category: "Tasks",
            execute: runStandalone("task.stop-active", () => stopTaskRun(runningTask!)),
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
                                          await refreshProject(activeProjectCwd);
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
        ...(activeKind === "project" && activeProjectCwd
            ? [
                  {
                      id: "project.settings",
                      title: "Project settings",
                      detail: "Tasks, the default git repository, worktrees and language servers",
                      category: "Project",
                      execute: runStandalone("project.settings", () => cmd.openProjectSettings()),
                  } satisfies StandaloneCommand,
                  languageServersAllowedHere
                      ? ({
                            id: "project.language-servers.stop",
                            title: "Stop language servers for this project",
                            detail: "Stop them now and do not start them again",
                            category: "Project · Language servers",
                            execute: runStandalone("project.language-servers.stop", () => {
                                cmd.setLanguageServerTrust(activeProjectCwd, false);
                                void lsp.stop(activeProjectCwd).catch(reportError("stop language servers"));
                            }),
                        } satisfies StandaloneCommand)
                      : ({
                            id: "project.language-servers.allow",
                            title: "Allow language servers for this project",
                            detail: "Start them when you open a file here",
                            category: "Project · Language servers",
                            execute: runStandalone("project.language-servers.allow", () => cmd.setLanguageServerTrust(activeProjectCwd, true)),
                        } satisfies StandaloneCommand),
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
            title: "Show welcome",
            detail: "Open the first-run screen",
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
            id: "app.quit-and-stop-everything",
            title: "Quit and Stop Everything",
            detail: "Stop every terminal, terminal agent and task, then quit. Plain Quit leaves them running.",
            category: "Application",
            shortcut: QUIT_AND_STOP_SHORTCUT ?? undefined,
            execute: runStandalone("app.quit-and-stop-everything", () => void quitAndStopEverything()),
        },
        {
            id: "agents.reload-manifests",
            title: "Reload agent manifests",
            detail: "Reload agent-state detection rules from disk",
            category: "Agents",
            execute: runStandalone("agents.reload-manifests", () => void agentDetectionApi.reload().catch(reportError("agent manifest reload"))),
        },
        ...worktreeCommands,
        ...taskCommands,
        ...projectCommands,
    ];

    return (
        <CommandPalette
            keybindingOverrides={keybindingOverrides}
            customCommands={customCommands}
            recentCommandKeys={recentCommandKeys}
            standaloneCommands={standaloneCommands}
            context={activeKind}
            onClose={cmd.closeCommandPalette}
            onExecute={cmd.noteRecentCommand}
            executeBuiltin={(id) => {
                if (runKeybindingAction(id, new KeyboardEvent("keydown"), getState())) return;
                notify("info", `${getKeybindingAction(id)?.label ?? id}: nothing here to act on`);
            }}
            executeCustom={(command) => {
                cmd.runCustomCommand(command);
            }}
        />
    );
}

/*
 * The photographic ground, and nothing at all without one. The element carries
 * a blur and a mask, so leaving it mounted costs a full-window composited layer
 * to paint a picture that is `none` in every build nobody has customised.
 */
function ShellBackdrop() {
    return useBackdropImage() ? <div className="shell-image" aria-hidden="true" /> : null;
}

/** A start that finishes within this shows only the window's ground, so it never flashes. */
const SHOW_BOOT_AFTER_MS = 400;
/** A start still going after this says what it waits on. */
const NAME_THE_WAIT_AFTER_MS = 3000;

/** Whether `after` milliseconds have passed while the app is still starting. */
function useBootDelay(ready: boolean, after: number): boolean {
    const [passed, setPassed] = useState(false);
    useEffect(() => {
        if (ready) return;
        const timer = setTimeout(() => setPassed(true), after);
        return () => clearTimeout(timer);
    }, [ready, after]);
    return passed;
}

export default function App() {
    useKeymap();
    usePinchZoom();
    useAgentNotifications();
    useVoiceDictation();
    useBrowserDownloads();
    useBrowserReveal();
    useSimulatorReveal();
    useBrowserStrips();
    useRailWidthVars();
    const [bootReady, setBootReady] = useState(false);
    const [bootIssue, setBootIssue] = useState<string | null>(null);
    const [coreAnswered, setCoreAnswered] = useState(false);
    const bootShown = useBootDelay(bootReady, SHOW_BOOT_AFTER_MS);
    const bootSlow = useBootDelay(bootReady, NAME_THE_WAIT_AFTER_MS);
    const sideRailVisible = useStore((s) => s.sideRailOpen);
    const tabRailVisible = useStore((s) => s.tabRailOpen);
    const activeSessionIsProject = useStore((s) => s.sessions[s.activeSessionId]?.kind === "project");
    useRailDock(sideRailVisible, true, ".side-rail", bootReady);
    useRailDock(tabRailVisible, activeSessionIsProject, ".tab-rail", bootReady);
    const pickerOpen = useStore((s) => s.pickerOpen);
    const agentPaletteOpen = useStore((s) => s.agentPaletteOpen);
    const filePaletteOpen = useStore((s) => s.filePaletteOpen);
    const newTabPaletteOpen = useStore((s) => s.newTabPaletteOpen);
    const installedPlugins = useInstalledPlugins();
    const iosSimulator = usePreference("iosSimulator");
    useEffect(() => {
        if (iosSimulator !== undefined) void simApi.offerToAgents(iosSimulator).catch(swallow("switch the iOS Simulator"));
    }, [iosSimulator]);
    const settingsOpen = useStore((s) => s.settingsOpen);
    const uiTextScale = usePreference("uiTextScale");
    useEffect(() => {
        if (uiTextScale !== undefined) document.documentElement.style.setProperty("--ui-text-scale", String(uiTextScale));
    }, [uiTextScale]);
    const windowOpacity = usePreference("windowOpacity");
    useEffect(() => {
        if (windowOpacity !== undefined) applyWindowOpacity(windowOpacity);
    }, [windowOpacity]);
    const windowBlur = usePreference("windowBlur");
    useEffect(() => {
        if (windowBlur !== undefined) cmd.applyWindowBlur(windowBlur);
    }, [windowBlur]);
    useEffect(() => watchTheme(), []);
    const terminalFontSize = usePreference("terminalFontSize");
    useEffect(() => {
        if (terminalFontSize !== undefined && terminalFontSize !== currentTerminalFontSize()) applyTerminalFontSize(terminalFontSize);
    }, [terminalFontSize]);
    const chatTextScale = usePreference("chatTextScale");
    useEffect(() => {
        if (chatTextScale !== undefined) applyChatTextScale(chatTextScale);
    }, [chatTextScale]);
    const editorTextScale = usePreference("editorTextScale");
    useEffect(() => {
        if (editorTextScale !== undefined) applyEditorTextScale(editorTextScale);
    }, [editorTextScale]);
    const paneStrength = useStore((s) => preferencesOf(s).paneStrength);
    useEffect(() => {
        document.documentElement.style.setProperty("--pane-strength", String(paneStrength));
    }, [paneStrength]);
    const commandPaletteOpen = useStore((s) => s.commandPaletteOpen);
    const themePaletteOpen = useStore((s) => s.themePaletteOpen);
    const commandPopup = useStore((s) => s.commandPopup);
    const sessionSwitcherOpen = useStore((s) => s.sessionSwitcher !== null);
    const onboardingOpen = useStore((s) => s.onboardingOpen);
    const diagnosticsOpen = useStore((s) => s.diagnosticsOpen);
    const whatsNewOpen = useStore((s) => s.whatsNewOpen);
    useOccludeNativeViews(
        pickerOpen ||
            agentPaletteOpen ||
            filePaletteOpen ||
            newTabPaletteOpen ||
            settingsOpen ||
            commandPaletteOpen ||
            themePaletteOpen ||
            sessionSwitcherOpen ||
            onboardingOpen ||
            Boolean(commandPopup),
    );

    useEffect(() => {
        let disposed = false;
        let unsub = () => {};
        let stopHosts = () => {};
        let stopPaneResources = () => {};
        let stopAccountSync = () => {};
        let stopWorkspaceSync = () => {};
        let bootFinished = false;
        let hydrationResult: HydrationResult | null = null;
        const bootSpan = performanceTelemetry.startTrace("startup.boot");
        const finishBoot = (outcome: "success" | "error" | "cancelled") => {
            if (bootFinished) return;
            bootFinished = true;
            const recorded = performanceTelemetry.endSpan(bootSpan, { outcome });
            if (recorded) performanceTelemetry.recordLatency("startup.boot", recorded.durationMs);
        };
        void pluginsApi.manifests().then(cmd.setPluginManifests, swallow("plugin manifests"));
        let accounts: AgentAccountBook | null | undefined;
        let workspace: SharedWorkspace | undefined;
        const fromCore = Promise.all([
            agentApi.accounts().catch(() => undefined),
            fetchCoreWorkspace(),
            agentApi.catalog(),
            coreCall<Preferences>("preferences").catch(() => null),
            coreCall<Palette>("theme").catch(() => null),
        ]);
        void fromCore.then(
            () => !disposed && setCoreAnswered(true),
            () => {},
        );
        Promise.all([invoke<BootInfo>("boot_init"), fromCore])
            .then(([boot, [book, shared, catalog, preferences, palette]]) => {
                if (disposed) return;
                cmd.takeAgentCatalog(catalog);
                if (preferences) showPreferences(preferences);
                accounts = book;
                workspace = shared;
                const hydrateSpan = performanceTelemetry.startSpan(bootSpan, "startup.hydrate");
                try {
                    cmd.setHome(boot.home);
                    hydrationResult = applyHydrate(boot.state, accounts, workspace);
                    const st = getState();
                    if (palette) wearPicked(palette);
                    startPageTheme();
                    applyWindowOpacity(preferencesOf(st).windowOpacity);
                    if (preferences) {
                        document.documentElement.style.setProperty("--ui-text-scale", String(preferences.uiTextScale));
                        applyTerminalFontSize(preferences.terminalFontSize);
                        applyChatTextScale(preferences.chatTextScale);
                        applyEditorTextScale(preferences.editorTextScale);
                    }
                    cmd.applyWindowBlur(preferencesOf(st).windowBlur);
                    if (hydrationAllowsPersistence(hydrationResult)) {
                        offerSavedSessions();
                        if (preferences && !preferences.onboardingComplete) cmd.openOnboarding();
                        else
                            void whatsNew()
                                .then(({ unseen }) => unseen && cmd.openWhatsNew())
                                .catch(swallow("ask whether there is news"));
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
                    stopPaneResources = releaseClosedPaneResources();
                    unsub = subscribePersist();
                    if (workspace) stopWorkspaceSync = startCoreWorkspace(workspace);
                    stopAccountSync = syncAgentAccounts(accounts);
                    setBootReady(true);
                    introduceNotifications();
                    void restoreCoreSessions();
                    stopHosts = startHosts();
                }
                finishBoot(disposed ? "cancelled" : writable ? "success" : "error");
            });
        return () => {
            disposed = true;
            finishBoot("cancelled");
            unsub();
            stopWorkspaceSync();
            stopAccountSync();
            stopHosts();
            stopPaneResources();
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

    useEffect(watchInbox, []);

    useEffect(() => watchSshLinks(), []);
    useEffect(() => watchTerminalTitles(), []);
    useEffect(() => watchRailNames(), []);
    useEffect(() => watchFaceStyle(), []);
    useEffect(() => watchPreferences(), []);
    useEffect(() => watchDrafts(), []);
    useEffect(() => watchLspClaims(), []);
    useEffect(() => watchAgentStatus(), []);
    /* Fetched ahead so the first chat opened draws at once; a failure here leaves opening one to fetch it again. */
    useEffect(() => whenIdle(() => void import("./chat/AgentSurface").catch(swallow("preload the chat surface"))), []);

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

    useEffect(() => onCoreChanged("git", onGitChanged), []);

    useEffect(() => (UPDATES_IN_APP ? followUpdates() : undefined), []);

    useEffect(() => {
        const clearTreeHover = () => {
            emit({ type: "tree-native-drag-hover", cwd: null, targetDir: null, highlightPath: null });
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
                showPathDropHover(null);
                clearTreeHover();
                return;
            }

            const point = nativeDropPoint(e.payload.position);
            const at = document.elementFromPoint(point.x, point.y) as HTMLElement | null;
            const pathTarget = pathDropSurface(at, point);

            if (e.payload.type === "enter" || e.payload.type === "over") {
                showPathDropHover(pathTarget);
                if (pathTarget) clearTreeHover();
                else emitTreeHover(at);
                return;
            }

            showPathDropHover(null);
            const dropped = e.payload.paths;
            if (!dropped || dropped.length === 0) {
                clearTreeHover();
                return;
            }
            const target = resolveTreeDropTarget(at);
            clearTreeHover();
            void fsapi.keepDroppedPaths(dropped).then((paths) => {
                if (pathTarget && dispatchPaths(pathTarget, paths)) return;
                if (target) dispatchFolder(target.dropEl, paths);
            }, reportError("Could not take the dropped files"));
        });
        return () => {
            showPathDropHover(null);
            void unlistenP.then((u) => u());
        };
    }, []);

    if (!bootReady) {
        return (
            <div className="shell boot-shell">
                {bootIssue ? (
                    <section className="boot-state" role="alert" aria-live="polite">
                        <img className="boot-state-mark" src={brandMark} alt="" />
                        <h1>Workspace protected</h1>
                        <p>{bootIssue}</p>
                        <button type="button" onClick={() => window.location.reload()}>
                            Reload safely
                        </button>
                    </section>
                ) : (
                    bootShown && (
                        <section className="boot-state" role="status" aria-live="polite" aria-label="Opening Sikemux">
                            <img className="boot-state-mark" src={brandMark} alt="" />
                            {bootSlow && (
                                <p className="boot-state-wait">{coreAnswered ? "Opening your workspace" : "Waiting for Sikemux to start"}</p>
                            )}
                        </section>
                    )
                )}
                <Toaster />
            </div>
        );
    }

    return (
        <div className="shell">
            <ShellBackdrop />
            <CliOpenBridge />
            <DeepLinkBridge />
            <HarnessBridge />
            <RemoteWorkspaceBridge />
            <RemoteChatBridge />
            <AgentNameBridge />
            <AgentQuestionBridge />
            <AgentBoardBridge />
            <GlobalSkillsBridge />
            <NetworkBridge />
            <ProjectBridge />
            <TopBar />
            <TabRow />
            <div className="body">
                <SideRail />
                {sideRailVisible && <RailResizer edge="start" />}
                {!sideRailVisible && (
                    <RailPeek edge="start">
                        <SideRail />
                    </RailPeek>
                )}
                <main className={`stage${settingsOpen ? " stage--settings" : ""}`}>
                    <Workspace />
                    {settingsOpen && (
                        <Suspense fallback={null}>
                            <SettingsPanel />
                        </Suspense>
                    )}
                </main>
                {activeSessionIsProject && <TabRail />}
                {tabRailVisible && activeSessionIsProject && <RailResizer edge="end" />}
                {!tabRailVisible && activeSessionIsProject && (
                    <RailPeek edge="end">
                        <TabRail />
                    </RailPeek>
                )}
            </div>
            {pickerOpen && <SeshPicker />}
            {agentPaletteOpen && <AgentPalette />}
            {filePaletteOpen && <FilePalette />}
            {newTabPaletteOpen && <NewTabPalette />}
            {installedPlugins.map(({ id, Overlay }) => (Overlay ? <Overlay key={id} /> : null))}
            {sessionSwitcherOpen && <SessionSwitcher />}
            {commandPaletteOpen && <ApplicationCommandPalette />}
            {themePaletteOpen && <ThemePalette />}
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
            <Suspense fallback={null}>
                {onboardingOpen && <Onboarding />}
                {diagnosticsOpen && <DiagnosticsOverlay />}
                {whatsNewOpen && <WhatsNewOverlay />}
            </Suspense>
            <AgentInstallHost />
            <DialogHost />
            <AccountDialogHost />
            <ShareHost />
            <BuddyPickerHost />
            <ImageViewer />
            <VoiceCaption />
            <PairingPrompt />
            <UpdateRequiredPrompt />
            <Toaster />
        </div>
    );
}
