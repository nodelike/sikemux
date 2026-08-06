import { useCallback, useEffect, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent, ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
    eventToKeybinding,
    findKeybindingConflict,
    KEYBINDING_ACTIONS,
    KEYBINDING_CATEGORIES,
    keybindingHasModifier,
    keybindingLabel,
    resolvedKeybinding,
    type KeybindingActionId,
    type KeybindingOverrides,
} from "../keybindings";
import { settingsApi } from "../api/settings";
import { prettyPath } from "../lib/paths";
import { IS_MACOS } from "../lib/platform";
import { notify, reportError } from "../state/toast";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { cloneTheme, newCustomThemeId, THEME_GROUPS, THEMES, THEMES_BY_ID, type Theme, type ThemeGroupKey } from "../themes";
import { IconCheck, IconClose, IconFolder, IconPencil, IconPlus, IconRefresh, IconSave, IconTrash } from "./Icons";
import type { CommandContext, CustomCommand, CustomCommandPlacement } from "../commands/registry";
import { requestAgentNotificationPermission } from "./AgentNotifications";
import type { AgentType } from "../state/types";

type Page = "general" | "appearance" | "keybindings" | "commands" | "agents" | "notifications" | "cli" | "cloud" | "about";

const PAGES: { id: Page; name: string; detail: string }[] = [
    { id: "general", name: "General", detail: "Projects and discovery" },
    { id: "appearance", name: "Appearance", detail: "Theme and window" },
    { id: "keybindings", name: "Keybindings", detail: "Commands and navigation" },
    { id: "commands", name: "Command deck", detail: "Your contextual actions" },
    { id: "agents", name: "Agents", detail: "Restore and status behavior" },
    { id: "notifications", name: "Notifications", detail: "Attention without noise" },
    { id: "cli", name: "CLI", detail: "Shell and editor integration" },
    { id: "cloud", name: "Cloud", detail: "Sign-in workspace" },
    { id: "about", name: "About", detail: "Updates and diagnostics" },
];

export function SettingsPanel() {
    const pinnedProjects = useStore((s) => s.pinnedProjects);
    const projectRoots = useStore((s) => s.projectRoots);
    const themeId = useStore((s) => s.themeId);
    const windowOpacity = useStore((s) => s.windowOpacity);
    const windowBlur = useStore((s) => s.windowBlur);
    const cloudBrowser = useStore((s) => s.cloudBrowser);
    const cloudBrowserShortcut = useStore((s) => s.cloudBrowserShortcut);
    const keybindingOverrides = useStore((s) => s.keybindingOverrides);
    const home = useStore((s) => s.home);
    const settingsBinding = resolvedKeybinding(keybindingOverrides, "settings.toggle");
    const closeSettingsHint = settingsBinding ? `Esc / ${keybindingLabel(settingsBinding)}` : "Esc";

    const [page, setPage] = useState<Page>("general");

    useEffect(() => {
        const onKey = (e: KeyboardEvent) => {
            if (e.key === "Escape") {
                e.preventDefault();
                cmd.closeSettings();
            }
        };
        window.addEventListener("keydown", onKey);
        return () => window.removeEventListener("keydown", onKey);
    }, []);

    const pretty = (p: string) => prettyPath(p, home);

    return (
        <div className="settings-pane" role="dialog" aria-modal="true" aria-label="Settings">
            <div className="settings-frame">
                <aside className="settings-rail">
                    <div className="settings-rail-head">
                        <span className="settings-logo-mark">S</span>
                        <span>
                            <span className="settings-logo-text">Sikemux</span>
                            <span className="settings-logo-tag">Preferences</span>
                        </span>
                    </div>

                    <p className="settings-rail-intro">A calmer workspace starts here.</p>

                    <nav className="settings-rail-list">
                        {PAGES.map((p) => (
                            <button
                                key={p.id}
                                className={`settings-rail-item${page === p.id ? " active" : ""}`}
                                onClick={() => setPage(p.id)}
                                type="button">
                                <span className="settings-rail-name">{p.name}</span>
                                <span className="settings-rail-detail">{p.detail}</span>
                            </button>
                        ))}
                    </nav>

                    <div className="settings-rail-foot">
                        <span className="settings-rail-path">Changes save automatically</span>
                        <button className="settings-close" onClick={cmd.closeSettings} title={`Close settings (${closeSettingsHint})`} type="button">
                            <IconClose size={12} /> Done
                        </button>
                    </div>
                </aside>

                <div className="settings-main">
                    <header className="settings-topbar">
                        <div>
                            <span className="settings-topbar-kicker">Preferences</span>
                            <span className="settings-topbar-title">Settings</span>
                        </div>
                        <button
                            className="settings-topbar-close"
                            onClick={cmd.closeSettings}
                            title={`Close settings (${closeSettingsHint})`}
                            aria-label="Close settings"
                            type="button">
                            <IconClose size={14} />
                        </button>
                    </header>

                    <div className="settings-scroll">
                        {page === "general" && (
                            <GeneralPage pinnedProjects={pinnedProjects} projectRoots={projectRoots} home={home} pretty={pretty} />
                        )}

                        {page === "appearance" && <AppearancePage themeId={themeId} windowOpacity={windowOpacity} windowBlur={windowBlur} />}

                        {page === "keybindings" && <KeybindingsPage overrides={keybindingOverrides} />}

                        {page === "commands" && <CommandsPage />}

                        {page === "agents" && <AgentsPage />}

                        {page === "notifications" && <NotificationsPage />}

                        {page === "cli" && <CliPage />}

                        {page === "cloud" && <CloudPage cloudBrowser={cloudBrowser} cloudBrowserShortcut={cloudBrowserShortcut} />}

                        {page === "about" && <AboutPage />}
                    </div>
                </div>
            </div>
        </div>
    );
}

const COMMAND_CONTEXT_OPTIONS: CommandContext[] = ["project", "command", "ssh", "aws", "rundeck", "bruno"];
const COMMAND_PLACEMENTS: CustomCommandPlacement[] = ["terminal", "split", "popup", "background", "replace"];

function blankCommand(): CustomCommand {
    return { id: `command-${Date.now().toString(36)}`, title: "", detail: "", command: "", contexts: [], placement: "terminal" };
}

function CommandsPage() {
    const commands = useStore((s) => s.customCommands);
    const [draft, setDraft] = useState<CustomCommand>(() => blankCommand());
    const save = () => {
        if (!draft.title.trim() || !draft.command.trim()) return;
        cmd.upsertCustomCommand({ ...draft, title: draft.title.trim(), detail: draft.detail.trim() });
        setDraft(blankCommand());
    };
    return (
        <SettingsPage name="command deck" deck="Trusted shell actions that appear beside every built-in Sikemux command.">
            <SettingsSection
                title="Custom actions"
                meta={`${commands.length} saved`}
                sub="Commands run with the active session as cwd and receive SIKEMUX_SESSION_* and SIKEMUX_PROJECT environment variables. They are unsandboxed—only add commands you trust.">
                <div className="custom-command-list">
                    {commands.map((item) => (
                        <button key={item.id} type="button" onClick={() => setDraft(item)}>
                            <span>{item.title}</span>
                            <small>
                                {item.placement} · {item.contexts.length ? item.contexts.join(", ") : "all contexts"}
                            </small>
                        </button>
                    ))}
                    {commands.length === 0 && (
                        <span className="settings-field-help">
                            No custom commands yet. Built-ins are already searchable with the command-deck shortcut.
                        </span>
                    )}
                </div>
            </SettingsSection>
            <SettingsSection
                title={commands.some((item) => item.id === draft.id) ? "Edit action" : "New action"}
                sub="Choose where output should live: a terminal tab, split, temporary popup, background toast, or replacement pane.">
                <div className="command-editor-grid">
                    <input
                        className="settings-input"
                        placeholder="Display name"
                        value={draft.title}
                        onChange={(e) => setDraft({ ...draft, title: e.target.value })}
                    />
                    <input
                        className="settings-input"
                        placeholder="What it does"
                        value={draft.detail}
                        onChange={(e) => setDraft({ ...draft, detail: e.target.value })}
                    />
                    <textarea
                        className="settings-input command-editor-source"
                        placeholder="shell command"
                        value={draft.command}
                        onChange={(e) => setDraft({ ...draft, command: e.target.value })}
                        spellCheck={false}
                    />
                    <select
                        className="settings-input"
                        value={draft.placement}
                        onChange={(e) => setDraft({ ...draft, placement: e.target.value as CustomCommandPlacement })}>
                        {COMMAND_PLACEMENTS.map((value) => (
                            <option key={value}>{value}</option>
                        ))}
                    </select>
                    <div className="command-contexts">
                        {COMMAND_CONTEXT_OPTIONS.map((context) => (
                            <label key={context}>
                                <input
                                    type="checkbox"
                                    checked={draft.contexts.includes(context)}
                                    onChange={(e) =>
                                        setDraft({
                                            ...draft,
                                            contexts: e.target.checked
                                                ? [...draft.contexts, context]
                                                : draft.contexts.filter((item) => item !== context),
                                        })
                                    }
                                />
                                {context}
                            </label>
                        ))}
                    </div>
                    <div className="command-editor-actions">
                        <button className="settings-btn" type="button" onClick={() => setDraft(blankCommand())}>
                            new
                        </button>
                        {commands.some((item) => item.id === draft.id) && (
                            <button
                                className="settings-btn danger"
                                type="button"
                                onClick={() => {
                                    cmd.deleteCustomCommand(draft.id);
                                    setDraft(blankCommand());
                                }}>
                                <IconTrash size={11} /> delete
                            </button>
                        )}
                        <button className="settings-btn primary" type="button" disabled={!draft.title.trim() || !draft.command.trim()} onClick={save}>
                            <IconSave size={11} /> save
                        </button>
                    </div>
                </div>
            </SettingsSection>
        </SettingsPage>
    );
}

function ToggleSetting({
    label,
    detail,
    checked,
    disabled = false,
    onChange,
}: {
    label: string;
    detail: string;
    checked: boolean;
    disabled?: boolean;
    onChange: (value: boolean) => void;
}) {
    return (
        <label className="experience-setting-row">
            <span>
                <b>{label}</b>
                <small>{detail}</small>
            </span>
            <input type="checkbox" checked={checked} disabled={disabled} onChange={(event) => onChange(event.target.checked)} />
        </label>
    );
}

function AgentsPage() {
    const restore = useStore((s) => s.restoreAgentTabs);
    const auto = useStore((s) => s.autoResumeAgents);
    const density = useStore((s) => s.railDensity);
    return (
        <SettingsPage name="agents" deck="Safe continuity and dense, non-color-only status signals.">
            <SettingsSection
                title="Restart behavior"
                sub="Only confirmed native agent session IDs are saved. Raw startup commands and terminal evidence never touch disk.">
                <ToggleSetting
                    label="Restore agent tabs"
                    detail="Bring resumable tabs back in a dormant, inert state."
                    checked={restore}
                    onChange={cmd.setRestoreAgentTabs}
                />
                <ToggleSetting
                    label="Auto-resume restored agents"
                    detail="Explicit opt-in: launch every restored agent after Sikemux starts."
                    checked={auto}
                    disabled={!restore}
                    onChange={cmd.setAutoResumeAgents}
                />
            </SettingsSection>
            <SettingsSection title="Rail density" sub="Compact mode fits more sessions while keeping state symbols visible.">
                <select className="settings-input" value={density} onChange={(e) => cmd.setRailDensity(e.target.value as "comfortable" | "compact")}>
                    <option value="comfortable">comfortable</option>
                    <option value="compact">compact</option>
                </select>
            </SettingsSection>
        </SettingsPage>
    );
}

function NotificationsPage() {
    const prefs = useStore((s) => s.notificationPreferences);
    const patch = cmd.patchNotificationPreferences;
    return (
        <SettingsPage name="notifications" deck="Agent attention, delayed just enough to avoid noisy state flicker.">
            <SettingsSection title="Delivery" sub="Blocked and completed signals are delayed and cancelled if the agent starts working again.">
                <ToggleSetting
                    label="Agent notifications"
                    detail="Show in-app alerts. Native banners are added when macOS permission is granted."
                    checked={prefs.enabled}
                    onChange={(enabled) => {
                        if (!enabled) patch({ enabled: false });
                        else {
                            patch({ enabled: true });
                            void requestAgentNotificationPermission()
                                .then((granted) => {
                                    if (!granted) notify("info", "In-app alerts are on; native notification permission was not granted");
                                })
                                .catch(reportError("notifications"));
                        }
                    }}
                />
                <ToggleSetting
                    label="Native banners only when unfocused"
                    detail="In-app completion alerts remain visible while you are using Sikemux."
                    checked={prefs.onlyWhenUnfocused}
                    onChange={(onlyWhenUnfocused) => patch({ onlyWhenUnfocused })}
                />
                <div className="settings-actions">
                    <button
                        className="settings-btn"
                        type="button"
                        onClick={() =>
                            void requestAgentNotificationPermission()
                                .then((granted) =>
                                    notify(
                                        granted ? "success" : "info",
                                        granted ? "Native agent banners enabled" : "macOS did not grant native banners; in-app alerts remain enabled",
                                    ),
                                )
                                .catch(reportError("notifications"))
                        }>
                        Enable native banners…
                    </button>
                    <small>In-app alerts do not require macOS permission.</small>
                </div>
                <ToggleSetting
                    label="Signal sounds"
                    detail="Short synthesized tones; no external audio files or network fetches."
                    checked={prefs.sounds}
                    onChange={(sounds) => patch({ sounds })}
                />
                <label className="settings-field-label" htmlFor="notification-sound-style">
                    signal tone
                </label>
                <select
                    id="notification-sound-style"
                    className="settings-input"
                    value={prefs.soundStyle}
                    disabled={!prefs.sounds}
                    onChange={(event) => patch({ soundStyle: event.target.value as "soft" | "bright" })}>
                    <option value="soft">soft · calm sine</option>
                    <option value="bright">bright · sharper triangle</option>
                </select>
                <label className="settings-field-label">delivery delay · {prefs.delayMs} ms</label>
                <input type="range" min="0" max="5000" step="50" value={prefs.delayMs} onChange={(e) => patch({ delayMs: Number(e.target.value) })} />
            </SettingsSection>
            <SettingsSection title="Quiet hours" sub="Cross-midnight ranges are supported and evaluated in local time.">
                <ToggleSetting
                    label="Enable quiet hours"
                    detail="Silence sounds and native notifications in this window."
                    checked={prefs.quietHoursEnabled}
                    onChange={(quietHoursEnabled) => patch({ quietHoursEnabled })}
                />
                <div className="quiet-hours">
                    <input
                        className="settings-input"
                        type="time"
                        value={prefs.quietHoursStart}
                        onChange={(e) => patch({ quietHoursStart: e.target.value })}
                    />
                    <span>to</span>
                    <input
                        className="settings-input"
                        type="time"
                        value={prefs.quietHoursEnd}
                        onChange={(e) => patch({ quietHoursEnd: e.target.value })}
                    />
                </div>
            </SettingsSection>
            <SettingsSection title="Agent types" sub="Mute individual agent integrations while keeping notifications enabled for the others.">
                {(
                    [
                        ["claude", "Claude"],
                        ["codex", "Codex"],
                        ["hermes", "Hermes"],
                        ["pi", "Pi"],
                        ["opencode", "OpenCode"],
                    ] as const satisfies readonly (readonly [AgentType, string])[]
                ).map(([type, label]) => {
                    const muted = prefs.mutedAgentTypes.includes(type);
                    return (
                        <ToggleSetting
                            key={type}
                            label={`Mute ${label}`}
                            detail={`Suppress blocked and completed notifications from ${label} agents.`}
                            checked={muted}
                            onChange={(nextMuted) =>
                                patch({
                                    mutedAgentTypes: nextMuted
                                        ? [...prefs.mutedAgentTypes, type]
                                        : prefs.mutedAgentTypes.filter((candidate) => candidate !== type),
                                })
                            }
                        />
                    );
                })}
            </SettingsSection>
        </SettingsPage>
    );
}

type CliInstallState = "unavailable" | "notInstalled" | "installed" | "outdated" | "conflict";

interface CliInstallStatus {
    state: CliInstallState;
    installDir: string;
    cliPath: string;
    editorPath: string;
    executable: string | null;
    pathConfigured: boolean;
    message: string;
}

function CliPage() {
    const [status, setStatus] = useState<CliInstallStatus | null>(null);
    const [busy, setBusy] = useState(false);
    const refresh = useCallback(() => invoke<CliInstallStatus>("cli_install_status").then(setStatus).catch(reportError("CLI status")), []);

    useEffect(() => {
        refresh();
    }, [refresh]);

    const install = async () => {
        setBusy(true);
        try {
            const next = await invoke<CliInstallStatus>("cli_install");
            setStatus(next);
            notify("success", next.pathConfigured ? "Sikemux CLI is ready" : "Sikemux CLI installed; add its directory to PATH");
        } catch (error) {
            reportError("CLI install")(error);
            await invoke<CliInstallStatus>("cli_install_status")
                .then(setStatus)
                .catch(() => undefined);
        } finally {
            setBusy(false);
        }
    };

    const stateLabel = status?.state.replace(/[A-Z]/g, (letter) => ` ${letter.toLowerCase()}`) ?? "checking";
    const installDisabled = !status || busy || status.state === "installed" || status.state === "unavailable" || status.state === "conflict";
    const buttonLabel = busy
        ? "Installing…"
        : status?.state === "outdated"
          ? "Update CLI"
          : status?.state === "installed"
            ? "Installed"
            : "Install CLI";

    return (
        <SettingsPage name="command line" deck="Open files and projects in the running Sikemux app, with editor-style wait semantics.">
            <SettingsSection title="Shell integration" meta={stateLabel} sub={status?.message ?? "Checking the packaged command-line integration…"}>
                <div className="cli-integration">
                    <div className="cli-integration-paths">
                        <span>
                            <b>Commands</b>
                            <code>sikemux</code>
                            <code>sikemux-editor</code>
                        </span>
                        <span>
                            <b>Install directory</b>
                            <code>{status?.installDir || "—"}</code>
                        </span>
                    </div>
                    <div className="cli-integration-actions">
                        <button className="settings-btn" type="button" disabled={busy} onClick={refresh}>
                            <IconRefresh size={11} /> Refresh
                        </button>
                        <button className="settings-btn primary" type="button" disabled={installDisabled} onClick={() => void install()}>
                            {status?.state === "installed" && <IconCheck size={11} />}
                            {buttonLabel}
                        </button>
                    </div>
                </div>
                {status?.state === "conflict" && (
                    <p className="settings-field-help cli-integration-warning">
                        Sikemux will not overwrite <em>{status.cliPath}</em> or <em>{status.editorPath}</em>. Move the existing file yourself, then
                        refresh.
                    </p>
                )}
                {status?.state === "installed" && !status.pathConfigured && (
                    <p className="settings-field-help">
                        Add <em>{status.installDir}</em> to your shell’s PATH. Sikemux never edits shell startup files automatically.
                    </p>
                )}
            </SettingsSection>
            <SettingsSection title="Usage" sub="Existing files open in an editor tab. Project directories focus or create their workspace.">
                <pre className="cli-usage">{`sikemux .\nsikemux src/App.tsx:42:5\nsikemux open --wait README.md\nEDITOR=sikemux-editor git commit`}</pre>
            </SettingsSection>
        </SettingsPage>
    );
}

function AboutPage() {
    const updateChannel = useStore((s) => s.updateChannel);
    return (
        <SettingsPage name="about" deck="Release details, first-run guidance, and redacted runtime health.">
            <SettingsSection
                title="Update channel"
                meta={updateChannel}
                sub="Stable follows the latest signed release. Preview follows the signed moving preview release.">
                <select
                    className="settings-input"
                    value={updateChannel}
                    onChange={(event) => cmd.setUpdateChannel(event.target.value as "stable" | "preview")}>
                    <option value="stable">stable</option>
                    <option value="preview">preview</option>
                </select>
            </SettingsSection>
            <SettingsSection title="Support deck" sub="These views are also searchable from the command deck.">
                <div className="about-actions">
                    <button
                        className="settings-btn"
                        onClick={() => {
                            cmd.closeSettings();
                            cmd.openWhatsNew();
                        }}>
                        What’s New
                    </button>
                    <button
                        className="settings-btn"
                        onClick={() => {
                            cmd.closeSettings();
                            cmd.openDiagnostics();
                        }}>
                        Runtime diagnostics
                    </button>
                    <button
                        className="settings-btn"
                        onClick={() => {
                            cmd.closeSettings();
                            cmd.openOnboarding();
                        }}>
                        Replay onboarding
                    </button>
                </div>
            </SettingsSection>
            <SettingsSection
                title="Session transfer"
                sub="Clipboard bundles exclude Bruno secrets, drafts, terminal history, environment values, and all startup commands. Imported agents are dormant.">
                <div className="about-actions">
                    <button className="settings-btn" onClick={() => void cmd.exportActiveSession().catch(reportError("session export"))}>
                        Copy active session
                    </button>
                    <button className="settings-btn" onClick={() => void cmd.importSessionFromClipboard().catch(reportError("session import"))}>
                        Import from clipboard
                    </button>
                </div>
            </SettingsSection>
        </SettingsPage>
    );
}

interface GeneralPageProps {
    pinnedProjects: Array<{ path: string }>;
    projectRoots: Array<{ path: string; depth: number }>;
    home: string;
    pretty: (p: string) => string;
}

function GeneralPage({ pinnedProjects, projectRoots, home, pretty }: GeneralPageProps) {
    const [pinnedDraftPath, setPinnedDraftPath] = useState("");
    const [rootDraftPath, setRootDraftPath] = useState("");
    const [rootDraftDepth, setRootDraftDepth] = useState(1);
    const inputRef = useRef<HTMLInputElement>(null);

    useEffect(() => {
        inputRef.current?.focus();
    }, []);

    const resolveDirectory = async (raw: string) => {
        const expanded = await settingsApi.expandPath(raw);
        const ok = await settingsApi.isDirectory(expanded);
        if (!ok) {
            notify("error", `settings: not a directory: ${pretty(expanded)}`);
            return null;
        }
        return expanded;
    };

    const commitPinnedDraft = async () => {
        const raw = pinnedDraftPath.trim();
        if (!raw) return;
        try {
            const expanded = await resolveDirectory(raw);
            if (!expanded) return;
            cmd.addPinnedProject(expanded);
            setPinnedDraftPath("");
        } catch (err) {
            reportError("settings")(err);
        }
    };

    const commitRootDraft = async () => {
        const raw = rootDraftPath.trim();
        if (!raw) return;
        try {
            const expanded = await resolveDirectory(raw);
            if (!expanded) return;
            cmd.addProjectRoot(expanded, rootDraftDepth);
            setRootDraftPath("");
            setRootDraftDepth(1);
        } catch (err) {
            reportError("settings")(err);
        }
    };

    const onPickPinned = async () => {
        try {
            const picked = await settingsApi.pickFolder(home || undefined);
            if (picked) cmd.addPinnedProject(picked);
        } catch (err) {
            reportError("folder picker")(err);
        }
    };

    const onPickRoot = async () => {
        try {
            const picked = await settingsApi.pickFolder(home || undefined);
            if (picked) cmd.addProjectRoot(picked, rootDraftDepth);
        } catch (err) {
            reportError("folder picker")(err);
        }
    };

    return (
        <SettingsPage name="general" deck="Exact project folders and git repo discovery for the session picker.">
            <SettingsSection
                title="Pinned projects"
                meta={`${pinnedProjects.length} ${pinnedProjects.length === 1 ? "entry" : "entries"}`}
                sub="Exact folders that always appear in the sesh picker, git repo or not.">
                <div className="settings-row-input compact">
                    <input
                        ref={inputRef}
                        className="settings-input"
                        placeholder="~/    or    /Users/me/scratch"
                        value={pinnedDraftPath}
                        onChange={(e) => setPinnedDraftPath(e.target.value)}
                        onKeyDown={(e) => {
                            if (e.key === "Enter") {
                                e.preventDefault();
                                void commitPinnedDraft();
                            } else if (e.key === "Escape") {
                                cmd.closeSettings();
                            }
                        }}
                        spellCheck={false}
                    />
                    <button className="settings-btn" onClick={onPickPinned} type="button" title="Browse…">
                        <IconFolder size={11} /> browse
                    </button>
                    <button
                        className="settings-btn primary"
                        onClick={() => void commitPinnedDraft()}
                        disabled={!pinnedDraftPath.trim()}
                        type="button">
                        <IconPlus size={11} /> add
                    </button>
                </div>

                {pinnedProjects.length === 0 ? (
                    <div className="settings-empty">
                        no pinned projects — use the folder button in the sesh picker, or add <code>~/</code> here
                    </div>
                ) : (
                    <div className="settings-list">
                        {pinnedProjects.map((project, i) => (
                            <div className="settings-list-row" key={project.path}>
                                <span className="settings-list-idx">{String(i + 1).padStart(2, "0")}</span>
                                <span className="settings-list-path">{pretty(project.path)}</span>
                                <button className="settings-row-x" onClick={() => cmd.removePinnedProject(project.path)} title="Remove" type="button">
                                    <IconClose size={11} />
                                </button>
                            </div>
                        ))}
                    </div>
                )}
            </SettingsSection>

            <SettingsSection
                title="Discovery roots"
                meta={`${projectRoots.length} ${projectRoots.length === 1 ? "entry" : "entries"}`}
                sub="Scans for git repos only. The root itself appears only when it is a git repo.">
                <div className="settings-row-input">
                    <input
                        className="settings-input"
                        placeholder="~/proj    or    /Users/me/work"
                        value={rootDraftPath}
                        onChange={(e) => setRootDraftPath(e.target.value)}
                        onKeyDown={(e) => {
                            if (e.key === "Enter") {
                                e.preventDefault();
                                void commitRootDraft();
                            } else if (e.key === "Escape") {
                                cmd.closeSettings();
                            }
                        }}
                        spellCheck={false}
                    />
                    <DepthStepper value={rootDraftDepth} onChange={setRootDraftDepth} title="Walk depth for this root" />
                    <button className="settings-btn" onClick={onPickRoot} type="button" title="Browse…">
                        <IconFolder size={11} /> browse
                    </button>
                    <button className="settings-btn primary" onClick={() => void commitRootDraft()} disabled={!rootDraftPath.trim()} type="button">
                        <IconPlus size={11} /> add
                    </button>
                </div>

                {projectRoots.length === 0 ? (
                    <div className="settings-empty">
                        no discovery roots — add <code>~/proj</code> (or wherever you keep code) to find git repos
                    </div>
                ) : (
                    <div className="settings-list">
                        {projectRoots.map((root, i) => (
                            <div className="settings-list-row" key={root.path}>
                                <span className="settings-list-idx">{String(i + 1).padStart(2, "0")}</span>
                                <span className="settings-list-path">{pretty(root.path)}</span>
                                <DepthStepper value={root.depth} onChange={(d) => cmd.setProjectRootDepth(root.path, d)} title="Walk depth" />
                                <button className="settings-row-x" onClick={() => cmd.removeProjectRoot(root.path)} title="Remove" type="button">
                                    <IconClose size={11} />
                                </button>
                            </div>
                        ))}
                    </div>
                )}
            </SettingsSection>
        </SettingsPage>
    );
}

function KeybindingsPage({ overrides }: { overrides: KeybindingOverrides }) {
    const [query, setQuery] = useState("");
    const [recording, setRecording] = useState<KeybindingActionId | null>(null);
    const [message, setMessage] = useState("");
    const normalizedQuery = query.trim().toLowerCase();
    const overrideCount = Object.keys(overrides).length;

    useEffect(() => {
        if (!recording) return;
        const cancelRecording = (event: KeyboardEvent) => {
            if (event.key !== "Escape") return;
            event.preventDefault();
            event.stopImmediatePropagation();
            setRecording(null);
            setMessage("Change cancelled.");
        };
        window.addEventListener("keydown", cancelRecording, { capture: true });
        return () => window.removeEventListener("keydown", cancelRecording, { capture: true });
    }, [recording]);

    const beginRecording = (id: KeybindingActionId) => {
        setRecording(id);
        setMessage("Press a shortcut. Backspace clears it; Escape cancels.");
    };

    const capture = (event: ReactKeyboardEvent<HTMLButtonElement>, id: KeybindingActionId) => {
        event.preventDefault();
        event.stopPropagation();

        if (event.key === "Escape") {
            setRecording(null);
            setMessage("Change cancelled.");
            return;
        }
        if (event.key === "Backspace" || event.key === "Delete") {
            cmd.setKeybinding(id, null);
            setRecording(null);
            setMessage(`${KEYBINDING_ACTIONS.find((action) => action.id === id)?.label} is now unassigned.`);
            return;
        }

        const binding = eventToKeybinding(event.nativeEvent);
        if (!binding) return;
        if (!keybindingHasModifier(binding)) {
            setMessage("Add Command, Control, Option, or Shift so typing stays safe.");
            return;
        }
        const conflict = findKeybindingConflict(overrides, id, binding);
        if (conflict) {
            setMessage(`${keybindingLabel(binding)} is already assigned to “${conflict.label}”.`);
            return;
        }

        cmd.setKeybinding(id, binding);
        setRecording(null);
        setMessage(`${KEYBINDING_ACTIONS.find((action) => action.id === id)?.label} changed to ${keybindingLabel(binding)}.`);
    };

    return (
        <SettingsPage name="keybindings" deck="Make the workspace move the way your hands already do. Changes apply instantly.">
            <SettingsSection
                title="Command map"
                meta={`${KEYBINDING_ACTIONS.length} commands · ${overrideCount} changed`}
                sub="Select a shortcut, then press a new combination. Conflicts are blocked so every command stays reachable.">
                <div className="keymap-toolbar">
                    <label className="keymap-search">
                        <span>filter</span>
                        <input
                            value={query}
                            onChange={(event) => setQuery(event.target.value)}
                            placeholder="panes, session, Bruno…"
                            spellCheck={false}
                        />
                    </label>
                    <button
                        className="settings-btn"
                        type="button"
                        disabled={overrideCount === 0}
                        onClick={() => {
                            cmd.resetAllKeybindings();
                            setRecording(null);
                            setMessage("All shortcuts restored to their defaults.");
                        }}>
                        <IconRefresh size={11} /> reset all
                    </button>
                </div>

                <div className={`keymap-status${recording ? " listening" : ""}`} aria-live="polite">
                    <span className="keymap-status-light" />
                    <span>{message || "Select any keycap to record a replacement."}</span>
                </div>

                <div className="keymap-groups">
                    {KEYBINDING_CATEGORIES.map((category) => {
                        const actions = KEYBINDING_ACTIONS.filter(
                            (action) =>
                                action.category === category &&
                                (!normalizedQuery ||
                                    `${action.label} ${action.detail} ${keybindingLabel(
                                        resolvedKeybinding(overrides, action.id as KeybindingActionId),
                                    )}`
                                        .toLowerCase()
                                        .includes(normalizedQuery)),
                        );
                        if (!actions.length) return null;
                        return (
                            <section className="keymap-group" key={category}>
                                <header className="keymap-group-head">
                                    <h3>{category}</h3>
                                    <span>{actions.length}</span>
                                </header>
                                <div className="keymap-list">
                                    {actions.map((action) => {
                                        const id = action.id as KeybindingActionId;
                                        const binding = resolvedKeybinding(overrides, id);
                                        const changed = Object.prototype.hasOwnProperty.call(overrides, id);
                                        const listening = recording === id;
                                        return (
                                            <div className={`keymap-row${listening ? " recording" : ""}`} key={id}>
                                                <div className="keymap-copy">
                                                    <span className="keymap-name">{action.label}</span>
                                                    <span className="keymap-detail">{action.detail}</span>
                                                </div>
                                                <div className="keymap-controls">
                                                    {changed && (
                                                        <button
                                                            className="keymap-reset"
                                                            type="button"
                                                            title={`Reset ${action.label}`}
                                                            aria-label={`Reset ${action.label}`}
                                                            onClick={() => {
                                                                cmd.resetKeybinding(id);
                                                                setMessage(`${action.label} restored to ${keybindingLabel(action.defaultBinding)}.`);
                                                            }}>
                                                            <IconRefresh size={10} />
                                                        </button>
                                                    )}
                                                    <button
                                                        className={`keymap-recorder${!binding ? " empty" : ""}${listening ? " listening" : ""}`}
                                                        type="button"
                                                        data-keybinding-recorder={listening ? "true" : undefined}
                                                        ref={(node) => {
                                                            if (listening) node?.focus();
                                                        }}
                                                        onClick={() => beginRecording(id)}
                                                        onKeyDown={(event) => capture(event, id)}
                                                        aria-label={`${action.label}: ${keybindingLabel(binding)}. Activate to change.`}>
                                                        {listening ? <span className="keymap-caret">press keys</span> : keybindingLabel(binding)}
                                                    </button>
                                                </div>
                                            </div>
                                        );
                                    })}
                                </div>
                            </section>
                        );
                    })}
                    {normalizedQuery &&
                        !KEYBINDING_ACTIONS.some((action) =>
                            `${action.label} ${action.detail} ${keybindingLabel(resolvedKeybinding(overrides, action.id as KeybindingActionId))}`
                                .toLowerCase()
                                .includes(normalizedQuery),
                        ) && <div className="settings-empty">no commands match “{query.trim()}”</div>}
                </div>

                <p className="keymap-foot">
                    {IS_MACOS
                        ? "macOS may keep system-reserved combinations before Sikemux can receive them."
                        : "Windows may keep system-reserved combinations before Sikemux can receive them."}
                </p>
            </SettingsSection>
        </SettingsPage>
    );
}

interface AppearancePageProps {
    themeId: string;
    windowOpacity: number;
    windowBlur: number;
}

interface ThemeEdit {
    theme: Theme;
    /** Pristine source the draft was forked from — used by "reset". */
    original: Theme;
    /** true ⇒ save inserts a new custom theme · false ⇒ overwrites an existing one. */
    isNew: boolean;
    baseName: string;
}

function AppearancePage({ themeId, windowOpacity, windowBlur }: AppearancePageProps) {
    const customThemes = useStore((s) => s.customThemes);
    const themeMode = useStore((s) => s.themeMode);
    const systemLightThemeId = useStore((s) => s.systemLightThemeId);
    const systemDarkThemeId = useStore((s) => s.systemDarkThemeId);
    const [edit, setEdit] = useState<ThemeEdit | null>(null);
    const editorRef = useRef<HTMLDivElement>(null);

    // Drive the whole-app live preview off the working draft; restore on close/unmount.
    useEffect(() => {
        if (edit) cmd.previewThemeDraft(edit.theme);
    }, [edit]);
    useEffect(() => () => cmd.cancelThemePreview(), []);

    const openEditor = (next: ThemeEdit) => {
        setEdit(next);
        requestAnimationFrame(() => editorRef.current?.scrollIntoView({ behavior: "smooth", block: "start" }));
    };

    const customizeFrom = (src: Theme) =>
        openEditor({
            theme: cloneTheme(src, { id: newCustomThemeId(), name: `${src.name} custom` }),
            original: cloneTheme(src),
            isNew: true,
            baseName: src.name,
        });

    const editCustom = (src: Theme) => openEditor({ theme: cloneTheme(src), original: cloneTheme(src), isNew: false, baseName: src.name });

    const newFromActive = () => customizeFrom(THEMES_BY_ID[themeId] ?? customThemes.find((t) => t.id === themeId) ?? THEMES[0]);

    const closeEditor = () => {
        setEdit(null);
        cmd.cancelThemePreview();
    };

    const saveEditor = () => {
        if (!edit) return;
        cmd.saveCustomTheme({ ...edit.theme, name: edit.theme.name.trim() || "custom theme" });
        setEdit(null);
    };

    const renderCard = (th: Theme, custom: boolean) => {
        const active = th.id === themeId;
        const editing = edit?.theme.id === th.id;
        return (
            <div key={th.id} className={`settings-theme${active ? " active" : ""}${editing ? " editing" : ""}`}>
                <button className="settings-theme-hit" onClick={() => cmd.setThemeId(th.id)} title={`Apply ${th.name}`} type="button">
                    <div className="settings-theme-preview" style={{ background: th.editor.bg, color: th.editor.fg }}>
                        <span className="settings-theme-preview-mark" style={{ color: th.chrome.acc }}>
                            Aa
                        </span>
                        <span className="settings-theme-preview-code" style={{ color: th.highlight.comment }}>
                            // make it yours
                        </span>
                        <span className="settings-theme-preview-accent" style={{ background: th.chrome.acc }} />
                    </div>
                    <div className="settings-theme-body">
                        <div className="settings-theme-name-row">
                            <span className="settings-theme-name">{th.name}</span>
                            {active && <span className="settings-theme-current">Current</span>}
                        </div>
                        <div className="settings-swatches">
                            <span style={{ background: th.terminal.red }} />
                            <span style={{ background: th.terminal.green }} />
                            <span style={{ background: th.terminal.yellow }} />
                            <span style={{ background: th.terminal.blue }} />
                            <span style={{ background: th.terminal.magenta }} />
                            <span style={{ background: th.terminal.cyan }} />
                        </div>
                    </div>
                </button>
                <div className="settings-theme-actions">
                    {custom ? (
                        <>
                            <button className="settings-theme-act" onClick={() => editCustom(th)} title="Edit theme" type="button">
                                <IconPencil size={11} />
                            </button>
                            <button
                                className="settings-theme-act danger"
                                onClick={() => cmd.deleteCustomTheme(th.id)}
                                title="Delete theme"
                                type="button">
                                <IconTrash size={11} />
                            </button>
                        </>
                    ) : (
                        <button className="settings-theme-act" onClick={() => customizeFrom(th)} title="Customize a copy" type="button">
                            <IconPencil size={11} />
                        </button>
                    )}
                </div>
                {custom && <span className="settings-theme-badge">custom</span>}
            </div>
        );
    };

    return (
        <SettingsPage
            name="appearance"
            deck={
                IS_MACOS
                    ? "Theme, window opacity and background blur. Changes apply instantly."
                    : "Theme and editor appearance. Changes apply instantly."
            }>
            <SettingsSection
                title="Host appearance"
                meta={themeMode}
                sub="Follow the operating system with Aura Day and your chosen dark cockpit, or keep one theme fixed.">
                <ToggleSetting
                    label="Follow system light/dark"
                    detail="Switches immediately when the host appearance changes."
                    checked={themeMode === "system"}
                    onChange={(enabled) => cmd.setThemeMode(enabled ? "system" : "manual")}
                />
                <div className="system-theme-grid">
                    <label>
                        <span>Light appearance</span>
                        <select
                            className="settings-input"
                            value={systemLightThemeId}
                            onChange={(event) => cmd.setSystemLightThemeId(event.target.value)}>
                            {[...THEMES, ...customThemes]
                                .filter((theme) => !theme.dark || theme.id === systemLightThemeId)
                                .map((theme) => (
                                    <option key={theme.id} value={theme.id}>
                                        {theme.name}
                                        {customThemes.some((candidate) => candidate.id === theme.id) ? " · custom" : ""}
                                    </option>
                                ))}
                        </select>
                    </label>
                    <label>
                        <span>Dark appearance</span>
                        <select
                            className="settings-input"
                            value={systemDarkThemeId}
                            onChange={(event) => cmd.setSystemDarkThemeId(event.target.value)}>
                            {[...THEMES, ...customThemes]
                                .filter((theme) => theme.dark || theme.id === systemDarkThemeId)
                                .map((theme) => (
                                    <option key={theme.id} value={theme.id}>
                                        {theme.name}
                                        {customThemes.some((candidate) => candidate.id === theme.id) ? " · custom" : ""}
                                    </option>
                                ))}
                        </select>
                    </label>
                </div>
            </SettingsSection>
            <SettingsSection
                title="Theme"
                meta={`${THEMES.length} built-in · ${customThemes.length} custom`}
                sub="Applies instantly to chrome, editor and terminal — no reload. Hover a swatch to customize or delete.">
                <div className="settings-theme-grid">{THEMES.map((th) => renderCard(th, false))}</div>

                {customThemes.length > 0 && (
                    <>
                        <div className="settings-theme-divider">your themes</div>
                        <div className="settings-theme-grid">{customThemes.map((th) => renderCard(th, true))}</div>
                    </>
                )}

                <div className="settings-theme-newrow">
                    <button className="settings-btn" onClick={newFromActive} type="button" title="Fork the active theme into a new editable copy">
                        <IconPlus size={11} /> new from current
                    </button>
                </div>
            </SettingsSection>

            {edit && (
                <div ref={editorRef}>
                    <ThemeEditor
                        edit={edit}
                        onColor={(group, key, value) =>
                            setEdit((e) =>
                                e
                                    ? {
                                          ...e,
                                          theme: { ...e.theme, [group]: { ...(e.theme[group] as unknown as Record<string, string>), [key]: value } },
                                      }
                                    : e,
                            )
                        }
                        onName={(name) => setEdit((e) => (e ? { ...e, theme: { ...e.theme, name } } : e))}
                        onDark={(dark) => setEdit((e) => (e ? { ...e, theme: { ...e.theme, dark } } : e))}
                        onReset={() => setEdit((e) => (e ? { ...e, theme: { ...cloneTheme(e.original), id: e.theme.id, name: e.theme.name } } : e))}
                        onSave={saveEditor}
                        onCancel={closeEditor}
                    />
                </div>
            )}

            {IS_MACOS && (
                <SettingsSection title="Window feel" sub="Tune the amount of glass without leaving this page.">
                    <div className="settings-control-stack">
                        <div className="settings-control">
                            <div className="settings-control-copy">
                                <h3>Opacity</h3>
                                <p>Solid at 1.00, translucent below it.</p>
                            </div>
                            <div className="settings-knob-row">
                                <input
                                    type="range"
                                    className="settings-slider"
                                    min={0}
                                    max={1}
                                    step={0.01}
                                    value={windowOpacity}
                                    onChange={(e) => cmd.setWindowOpacity(parseFloat(e.target.value))}
                                />
                                <NumberField value={windowOpacity} onCommit={cmd.setWindowOpacity} format={(v) => v.toFixed(2)} suffix="opacity" />
                            </div>
                        </div>
                        <div className="settings-control">
                            <div className="settings-control-copy">
                                <h3>Background blur</h3>
                                <p>0 is crisp; 20–40px gives a soft frosted effect.</p>
                            </div>
                            <div className="settings-knob-row">
                                <input
                                    type="range"
                                    className="settings-slider alt"
                                    min={0}
                                    max={60}
                                    step={1}
                                    value={Math.min(60, windowBlur)}
                                    onChange={(e) => cmd.setWindowBlur(parseInt(e.target.value, 10))}
                                />
                                <NumberField
                                    value={windowBlur}
                                    onCommit={(v) => cmd.setWindowBlur(Math.round(v))}
                                    format={(v) => String(Math.round(v))}
                                    suffix="px"
                                />
                            </div>
                        </div>
                    </div>
                </SettingsSection>
            )}
        </SettingsPage>
    );
}

interface ThemeEditorProps {
    edit: ThemeEdit;
    onColor: (group: ThemeGroupKey, key: string, value: string) => void;
    onName: (name: string) => void;
    onDark: (dark: boolean) => void;
    onReset: () => void;
    onSave: () => void;
    onCancel: () => void;
}

function ThemeEditor({ edit, onColor, onName, onDark, onReset, onSave, onCancel }: ThemeEditorProps) {
    const { theme, isNew, baseName } = edit;
    return (
        <section className="theme-editor">
            <header className="theme-editor-head">
                <div className="theme-editor-title">
                    <span className="theme-editor-kicker">{isNew ? "new theme" : "editing"}</span>
                    <input
                        className="theme-editor-name"
                        value={theme.name}
                        spellCheck={false}
                        placeholder="theme name"
                        onChange={(e) => onName(e.target.value)}
                        autoFocus
                    />
                    <span className="theme-editor-base">based on {baseName}</span>
                </div>
                <div className="theme-editor-tools">
                    <button
                        className={`theme-mode-toggle${theme.dark ? " dark" : " light"}`}
                        onClick={() => onDark(!theme.dark)}
                        type="button"
                        title="Editor light/dark hint — affects CodeMirror defaults">
                        {theme.dark ? "dark" : "light"}
                    </button>
                    <button className="settings-btn" onClick={onReset} type="button" title="Revert all colours to the source theme">
                        reset
                    </button>
                    <button className="settings-btn" onClick={onCancel} type="button">
                        <IconClose size={11} /> cancel
                    </button>
                    <button className="settings-btn primary" onClick={onSave} type="button">
                        {isNew ? <IconSave size={11} /> : <IconCheck size={11} />} {isNew ? "save theme" : "update"}
                    </button>
                </div>
            </header>

            <ThemePreview theme={theme} />

            <div className="theme-editor-groups">
                {THEME_GROUPS.map((group) => (
                    <div className="theme-group" key={group.key}>
                        <div className="theme-group-head">
                            <h3 className="theme-group-title">{group.label}</h3>
                            <span className="theme-group-hint">{group.hint}</span>
                        </div>
                        <div className="theme-group-grid">
                            {group.fields.map((field) => (
                                <ColorField
                                    key={field.key}
                                    label={field.label}
                                    value={(theme[group.key] as unknown as Record<string, string>)[field.key]}
                                    onChange={(v) => onColor(group.key, field.key, v)}
                                />
                            ))}
                        </div>
                    </div>
                ))}
            </div>
            <p className="theme-editor-foot">
                Hex or any CSS colour works in the text box — use <em>rgba(…)</em> for translucent washes. The picker only sets hex.
            </p>
        </section>
    );
}

const HEX6 = /^#([0-9a-fA-F]{6})$/;
const HEX3 = /^#([0-9a-fA-F]{3})$/;
const RGB = /rgba?\(\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)/i;

/** Best-effort projection of any CSS colour string onto a #rrggbb value for the native colour input. */
function toHex(value: string): string {
    const v = value.trim();
    const m6 = HEX6.exec(v);
    if (m6) return `#${m6[1].toLowerCase()}`;
    const m3 = HEX3.exec(v);
    if (m3) {
        const [r, g, b] = m3[1].split("");
        return `#${r}${r}${g}${g}${b}${b}`.toLowerCase();
    }
    const rgb = RGB.exec(v);
    if (rgb) {
        const h = (n: string) =>
            Math.max(0, Math.min(255, Math.round(parseFloat(n))))
                .toString(16)
                .padStart(2, "0");
        return `#${h(rgb[1])}${h(rgb[2])}${h(rgb[3])}`;
    }
    return "#000000";
}

function ColorField({ label, value, onChange }: { label: string; value: string; onChange: (v: string) => void }) {
    return (
        <div className="theme-field">
            <label className="theme-field-swatch" style={{ background: value }} title={`${label}: ${value}`}>
                <input type="color" value={toHex(value)} onChange={(e) => onChange(e.target.value)} />
            </label>
            <div className="theme-field-body">
                <span className="theme-field-label">{label}</span>
                <input className="theme-field-hex" value={value} spellCheck={false} onChange={(e) => onChange(e.target.value)} />
            </div>
        </div>
    );
}

function ThemePreview({ theme }: { theme: Theme }) {
    const h = theme.highlight;
    const ansi = [
        theme.terminal.black,
        theme.terminal.red,
        theme.terminal.green,
        theme.terminal.yellow,
        theme.terminal.blue,
        theme.terminal.magenta,
        theme.terminal.cyan,
        theme.terminal.white,
        theme.terminal.brightBlack,
        theme.terminal.brightRed,
        theme.terminal.brightGreen,
        theme.terminal.brightYellow,
        theme.terminal.brightBlue,
        theme.terminal.brightMagenta,
        theme.terminal.brightCyan,
        theme.terminal.brightWhite,
    ];
    return (
        <div className="theme-preview">
            <pre className="theme-preview-code" style={{ background: theme.editor.bg, color: theme.editor.fg }}>
                <span style={{ color: h.comment, fontStyle: "italic" }}>{"// fork a base, tweak, save"}</span>
                {"\n"}
                <span style={{ color: h.keyword }}>const</span> <span style={{ color: h.variable }}>swatch</span>
                <span style={{ color: h.operator }}> = </span>
                <span style={{ color: h.function }}>paint</span>
                <span style={{ color: h.operator }}>(</span>
                <span style={{ color: h.string }}>"#a277ff"</span>
                <span style={{ color: h.operator }}>, </span>
                <span style={{ color: h.number }}>0.3</span>
                <span style={{ color: h.operator }}>);</span>
            </pre>
            <div className="theme-preview-term" style={{ background: theme.terminal.background }}>
                {ansi.map((c, i) => (
                    <span key={i} style={{ background: c }} />
                ))}
            </div>
        </div>
    );
}

interface CloudPageProps {
    cloudBrowser: string;
    cloudBrowserShortcut: string;
}

function CloudPage({ cloudBrowser, cloudBrowserShortcut }: CloudPageProps) {
    return (
        <SettingsPage name="cloud" deck="Where AWS / GCP single sign-on URLs open, and which workspace to bounce to.">
            <SettingsSection title="Sign-in browser" meta="aws · gcp · sso" sub="Where the SSO URL lands. Pick the app you actually log in with.">
                <label className="settings-field-label">browser app</label>
                <input
                    className="settings-input wide"
                    placeholder="e.g. Zen, Arc, Safari · empty = system default"
                    value={cloudBrowser}
                    onChange={(e) => cmd.setCloudBrowser(e.target.value)}
                    spellCheck={false}
                />
                <div className="settings-field-help">must match a running app's name · trailing .app is fine</div>
            </SettingsSection>

            <SettingsSection
                title="Workspace switch"
                meta="optional"
                sub="Fired right after the link opens — point it at the desktop where the browser lives.">
                <label className="settings-field-label">workspace shortcut</label>
                <input
                    className="settings-input wide"
                    placeholder="e.g. ctrl+3 · empty = no switch"
                    value={cloudBrowserShortcut}
                    onChange={(e) => cmd.setCloudBrowserShortcut(e.target.value)}
                    spellCheck={false}
                />
                <div className="settings-field-help">
                    format: <em>mod+key</em> · use system shortcuts from Mission Control
                </div>
            </SettingsSection>
        </SettingsPage>
    );
}

function SettingsPage({ name, deck, children }: { name: string; deck: ReactNode; children: ReactNode }) {
    return (
        <div className="settings-page">
            <header className="settings-page-head">
                <h1 className="settings-page-hd">{name}</h1>
                <p className="settings-page-deck">{deck}</p>
            </header>
            {children}
        </div>
    );
}

function SettingsSection({ title, meta, sub, children }: { title: ReactNode; meta?: ReactNode; sub?: ReactNode; children: ReactNode }) {
    return (
        <section className="settings-section">
            <div className="settings-section-head">
                <h2 className="settings-section-title">{title}</h2>
                {meta && <span className="settings-section-meta">{meta}</span>}
            </div>
            {sub && <p className="settings-section-sub">{sub}</p>}
            {children}
        </section>
    );
}

interface NumberFieldProps {
    value: number;
    onCommit: (v: number) => void;
    format: (v: number) => string;
    suffix?: string;
}

function NumberField({ value, onCommit, format, suffix }: NumberFieldProps) {
    const [draft, setDraft] = useState<string>(() => format(value));
    const focusedRef = useRef(false);

    useEffect(() => {
        if (!focusedRef.current) setDraft(format(value));
    }, [value, format]);

    const commit = () => {
        const n = parseFloat(draft);
        if (Number.isFinite(n)) {
            onCommit(n);
            setDraft(format(n));
        } else {
            setDraft(format(value));
        }
    };

    return (
        <div className="settings-knob-num">
            <input
                type="text"
                inputMode="decimal"
                className="settings-knob-val"
                value={draft}
                spellCheck={false}
                onFocus={() => {
                    focusedRef.current = true;
                }}
                onBlur={() => {
                    focusedRef.current = false;
                    commit();
                }}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                    if (e.key === "Enter") {
                        e.preventDefault();
                        (e.target as HTMLInputElement).blur();
                    } else if (e.key === "Escape") {
                        setDraft(format(value));
                        (e.target as HTMLInputElement).blur();
                    }
                }}
            />
            {suffix && <span className="settings-knob-suf">{suffix}</span>}
        </div>
    );
}

function DepthStepper({ value, onChange, title }: { value: number; onChange: (v: number) => void; title?: string }) {
    const [draft, setDraft] = useState<string>(() => String(value));
    const focusedRef = useRef(false);

    useEffect(() => {
        if (!focusedRef.current) setDraft(String(value));
    }, [value]);

    const commit = (raw: string) => {
        const n = parseInt(raw, 10);
        if (Number.isFinite(n)) {
            const clamped = Math.max(0, n);
            onChange(clamped);
            setDraft(String(clamped));
        } else {
            setDraft(String(value));
        }
    };

    const bump = (delta: number) => {
        const next = Math.max(0, value + delta);
        onChange(next);
        setDraft(String(next));
    };

    return (
        <div className="settings-depth" title={title}>
            <span className="settings-depth-label">d</span>
            <button className="settings-depth-btn" onClick={() => bump(-1)} disabled={value <= 0} type="button">
                −
            </button>
            <input
                type="text"
                inputMode="numeric"
                className="settings-depth-input"
                value={draft}
                spellCheck={false}
                onFocus={() => {
                    focusedRef.current = true;
                }}
                onBlur={() => {
                    focusedRef.current = false;
                    commit(draft);
                }}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                    if (e.key === "Enter") {
                        e.preventDefault();
                        (e.target as HTMLInputElement).blur();
                    } else if (e.key === "Escape") {
                        setDraft(String(value));
                        (e.target as HTMLInputElement).blur();
                    } else if (e.key === "ArrowUp") {
                        e.preventDefault();
                        bump(1);
                    } else if (e.key === "ArrowDown") {
                        e.preventDefault();
                        bump(-1);
                    }
                }}
            />
            <button className="settings-depth-btn" onClick={() => bump(1)} type="button">
                +
            </button>
        </div>
    );
}
