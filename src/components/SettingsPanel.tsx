import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent, ReactNode } from "react";
import { invokeCommand as invoke } from "../api/invoke";
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
import { isUpdateBusy, updateCheckLabel } from "../api/updater";
import { prettyPath } from "../lib/paths";
import { IS_MACOS } from "../lib/platform";
import { notify, reportError } from "../state/toast";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { cloneTheme, newCustomThemeId, THEME_GROUPS, THEMES, THEMES_BY_ID, type Theme, type ThemeGroupKey } from "../themes";
import { IconCheck, IconClose, IconFolder, IconPencil, IconPlus, IconRefresh, IconSave, IconSearch, IconTrash } from "./Icons";
import {
    firstMatchingPage,
    searchSettings,
    settingsPage,
    settingsSection,
    SETTINGS_GROUPS,
    type SettingsMatches,
    type SettingsPageId,
    type SettingsSectionId,
} from "./settingsCatalog";
import { Dropdown, type DropdownOption } from "./Dropdown";
import { Checkbox, Slider, Switch } from "./Controls";
import { EmptyState } from "./Panel";
import { Tooltip } from "./Tooltip";
import type { CommandContext, CustomCommand, CustomCommandPlacement } from "../commands/registry";
import type { AgentProvider, ProjectRoot, ProviderProfile } from "../state/types";
import { AGENT_PERMISSION_COPY, AGENT_PERMISSION_MODES } from "../agentLaunch";

interface SettingsSearch {
    query: string;
    matches: SettingsMatches;
}

const SearchContext = createContext<SettingsSearch>({ query: "", matches: searchSettings("", IS_MACOS) });

const useSettingsSearch = () => useContext(SearchContext);

export function SettingsPanel() {
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

    const [page, setPage] = useState<SettingsPageId>("general");
    const [query, setQuery] = useState("");
    const railRef = useRef<HTMLElement>(null);
    const searchRef = useRef<HTMLInputElement>(null);

    const matches = useMemo(() => searchSettings(query, IS_MACOS), [query]);
    const searching = query.trim().length > 0;
    const groups = useMemo(
        () =>
            SETTINGS_GROUPS.map((group) => ({
                label: group.label,
                pages: group.pages.filter((candidate) => !searching || (matches.counts[candidate.id] ?? 0) > 0),
            })).filter((group) => group.pages.length > 0),
        [matches, searching],
    );
    const search = useMemo<SettingsSearch>(() => ({ query, matches }), [query, matches]);

    useEffect(() => {
        searchRef.current?.focus();
    }, []);

    useEffect(() => {
        if (!searching || (matches.counts[page] ?? 0) > 0) return;
        const next = firstMatchingPage(matches);
        if (next) setPage(next);
    }, [matches, page, searching]);

    useEffect(() => {
        const onKey = (e: KeyboardEvent) => {
            if (e.key !== "Escape") return;
            e.preventDefault();
            if (query) {
                setQuery("");
                searchRef.current?.focus();
                return;
            }
            cmd.closeSettings();
        };
        window.addEventListener("keydown", onKey);
        return () => window.removeEventListener("keydown", onKey);
    }, [query]);

    const pretty = (p: string) => prettyPath(p, home);

    const focusPage = (next: SettingsPageId) => {
        setPage(next);
        railRef.current?.querySelector<HTMLButtonElement>(`[data-settings-page="${next}"]`)?.focus();
    };

    const stepPage = (delta: number) => {
        const order = groups.flatMap((group) => group.pages);
        const index = order.findIndex((candidate) => candidate.id === page);
        const next = order[Math.max(0, Math.min(order.length - 1, index + delta))];
        if (next && next.id !== page) focusPage(next.id);
    };

    const onSearchKey = (e: ReactKeyboardEvent<HTMLInputElement>) => {
        if (e.key !== "ArrowDown" && e.key !== "Enter") return;
        const first = groups[0]?.pages[0];
        if (!first) return;
        e.preventDefault();
        focusPage(first.id);
    };

    return (
        <div className="settings-pane" role="dialog" aria-modal="true" aria-label="Settings">
            <div className="settings-frame">
                <aside className="settings-rail">
                    <div className="settings-search">
                        <IconSearch size={12} />
                        <input
                            ref={searchRef}
                            className="settings-search-input"
                            type="search"
                            value={query}
                            placeholder="Search settings"
                            aria-label="Search settings"
                            spellCheck={false}
                            onChange={(e) => setQuery(e.target.value)}
                            onKeyDown={onSearchKey}
                        />
                        {searching && (
                            <button className="settings-search-clear" type="button" aria-label="Clear search" onClick={() => setQuery("")}>
                                <IconClose size={10} />
                            </button>
                        )}
                    </div>

                    <nav
                        className="settings-rail-list"
                        aria-label="Settings sections"
                        ref={railRef}
                        onKeyDown={(e) => {
                            if (e.key === "ArrowDown") {
                                e.preventDefault();
                                stepPage(1);
                            } else if (e.key === "ArrowUp") {
                                e.preventDefault();
                                stepPage(-1);
                            }
                        }}>
                        {groups.map((group) => (
                            <div className="settings-rail-group" key={group.label}>
                                <span className="settings-rail-group-label">{group.label}</span>
                                {group.pages.map((candidate) => (
                                    <button
                                        key={candidate.id}
                                        className={`settings-rail-item${page === candidate.id ? " active" : ""}`}
                                        data-settings-page={candidate.id}
                                        aria-current={page === candidate.id ? "page" : undefined}
                                        onClick={() => setPage(candidate.id)}
                                        type="button">
                                        <span className="settings-rail-name">{candidate.name}</span>
                                        <span className="settings-rail-detail">{candidate.detail}</span>
                                        {searching && <span className="settings-rail-count">{matches.counts[candidate.id]}</span>}
                                    </button>
                                ))}
                            </div>
                        ))}
                        {groups.length === 0 && <p className="settings-rail-empty">No settings match “{query.trim()}”.</p>}
                    </nav>

                    <div className="settings-rail-foot">
                        <span className="settings-rail-path">Changes save automatically</span>
                    </div>
                </aside>

                <div className="settings-main">
                    <header className="settings-topbar">
                        <div>
                            <span className="settings-topbar-kicker">Settings</span>
                            <h1 className="settings-topbar-title">{settingsPage(page).name}</h1>
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
                        <SearchContext.Provider value={search}>
                            {groups.length === 0 ? (
                                <div className="settings-page">
                                    <EmptyState
                                        icon={<IconSearch size={14} />}
                                        title="Nothing matches that"
                                        message="Try a shorter word — shortcuts, theme, provider, update."
                                    />
                                </div>
                            ) : (
                                <>
                                    {page === "general" && <GeneralPage projectRoots={projectRoots} home={home} pretty={pretty} />}

                                    {page === "appearance" && (
                                        <AppearancePage themeId={themeId} windowOpacity={windowOpacity} windowBlur={windowBlur} />
                                    )}

                                    {page === "keybindings" && <KeybindingsPage overrides={keybindingOverrides} />}

                                    {page === "commands" && <CommandsPage />}

                                    {page === "agents" && <AgentsPage />}

                                    {page === "cli" && <CliPage />}

                                    {page === "cloud" && <CloudPage cloudBrowser={cloudBrowser} cloudBrowserShortcut={cloudBrowserShortcut} />}

                                    {page === "about" && <AboutPage />}
                                </>
                            )}
                        </SearchContext.Provider>
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
    const isSaved = commands.some((item) => item.id === draft.id);
    const save = () => {
        if (!draft.title.trim() || !draft.command.trim()) return;
        cmd.upsertCustomCommand({ ...draft, title: draft.title.trim(), detail: draft.detail.trim() });
        setDraft(blankCommand());
    };
    return (
        <SettingsPage page="commands">
            <SettingsSection id="commands.list" meta={`${commands.length} saved`}>
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
            <SettingsSection id="commands.editor" meta={isSaved ? "editing" : "new"}>
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
                    <Dropdown
                        className="settings-dd"
                        label="placement"
                        value={draft.placement}
                        options={COMMAND_PLACEMENTS.map((value) => ({ value, label: value }))}
                        onChange={(value) => setDraft({ ...draft, placement: value as CustomCommandPlacement })}
                    />
                    <div className="command-contexts">
                        {COMMAND_CONTEXT_OPTIONS.map((context) => (
                            <Checkbox
                                key={context}
                                checked={draft.contexts.includes(context)}
                                onChange={(on) =>
                                    setDraft({
                                        ...draft,
                                        contexts: on ? [...draft.contexts, context] : draft.contexts.filter((item) => item !== context),
                                    })
                                }>
                                {context}
                            </Checkbox>
                        ))}
                    </div>
                    <div className="command-editor-actions">
                        <button className="settings-btn" type="button" onClick={() => setDraft(blankCommand())}>
                            new
                        </button>
                        {isSaved && (
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
            <Switch checked={checked} disabled={disabled} onChange={onChange} label={label} />
        </label>
    );
}

function AgentsPage() {
    const restore = useStore((s) => s.restoreAgentTabs);
    const density = useStore((s) => s.railDensity);
    const profiles = useStore((s) => s.providerProfiles);
    const selectedProfiles = useStore((s) => s.selectedProviderProfileIds);
    const defaultPermissionMode = useStore((s) => s.defaultAgentPermissionMode);
    const [draft, setDraft] = useState<ProviderProfile>(
        () =>
            profiles[0] ?? {
                id: `profile-${Date.now().toString(36)}`,
                name: "Local provider",
                provider: "claude",
                accent: "#d97757",
            },
    );
    const isSaved = profiles.some((profile) => profile.id === draft.id);
    const newProfile = () =>
        setDraft({ id: `profile-${Date.now().toString(36)}`, name: "", provider: "claude", accent: "#d97757", environmentKeys: [] });
    return (
        <SettingsPage page="agents">
            <SettingsSection id="agents.safety" meta={AGENT_PERMISSION_COPY[defaultPermissionMode].label}>
                <div className="agent-mode-settings" role="radiogroup" aria-label="Default agent safety boundary">
                    {AGENT_PERMISSION_MODES.map((mode) => {
                        const copy = AGENT_PERMISSION_COPY[mode];
                        return (
                            <button
                                key={mode}
                                type="button"
                                role="radio"
                                aria-checked={defaultPermissionMode === mode}
                                className={`${defaultPermissionMode === mode ? "active" : ""} ${copy.tone}`}
                                onClick={() => cmd.setDefaultAgentPermissionMode(mode)}>
                                <span>{copy.label}</span>
                                <small>{copy.detail}</small>
                            </button>
                        );
                    })}
                </div>
            </SettingsSection>
            <SettingsSection id="agents.profiles" meta={`${profiles.length} configured`}>
                <div className="provider-profile-layout">
                    <div className="provider-profile-list">
                        {profiles.map((profile) => (
                            <button
                                key={profile.id}
                                type="button"
                                className={draft.id === profile.id ? "active" : ""}
                                onClick={() => setDraft({ ...profile, environmentKeys: [...(profile.environmentKeys ?? [])] })}>
                                <i style={{ background: profile.accent }} />
                                <span>
                                    <b>{profile.name}</b>
                                    <small>
                                        {profile.provider} · {profile.executablePath || "system PATH"}
                                    </small>
                                </span>
                            </button>
                        ))}
                        <button type="button" className="provider-profile-new" onClick={newProfile}>
                            <IconPlus size={11} /> new profile
                        </button>
                    </div>
                    <div className="provider-profile-editor">
                        <div className="provider-profile-row two">
                            <label>
                                <span>name</span>
                                <input
                                    className="settings-input"
                                    value={draft.name}
                                    onChange={(event) => setDraft({ ...draft, name: event.target.value })}
                                />
                            </label>
                            <label>
                                <span>provider</span>
                                <Dropdown
                                    className="settings-dd"
                                    label="provider"
                                    value={draft.provider}
                                    disabled={draft.id.startsWith("builtin-")}
                                    options={[
                                        { value: "claude", label: "Claude" },
                                        { value: "codex", label: "Codex" },
                                        { value: "gemini", label: "Gemini" },
                                    ]}
                                    onChange={(value) => setDraft({ ...draft, provider: value as AgentProvider })}
                                />
                            </label>
                        </div>
                        <label>
                            <span>executable path</span>
                            <input
                                className="settings-input"
                                placeholder="Leave empty to use PATH"
                                value={draft.executablePath ?? ""}
                                onChange={(event) => setDraft({ ...draft, executablePath: event.target.value || undefined })}
                            />
                        </label>
                        {(draft.provider === "claude" || draft.provider === "codex") && (
                            <label>
                                <span>profile directory</span>
                                <input
                                    className="settings-input"
                                    placeholder={draft.provider === "codex" ? "Automatic, or ~/.codex-work" : "Automatic, or ~/.claude-work"}
                                    value={draft.configPath ?? ""}
                                    onChange={(event) => setDraft({ ...draft, configPath: event.target.value || undefined })}
                                />
                                <small className="settings-field-help">
                                    Leave empty to use the CLI default. Set this only when keeping multiple {draft.provider} accounts side by side.
                                </small>
                            </label>
                        )}
                        <div className="command-editor-actions">
                            {isSaved && !draft.id.startsWith("builtin-") && (
                                <button
                                    className="settings-btn danger"
                                    type="button"
                                    onClick={() => {
                                        cmd.deleteProviderProfile(draft.id);
                                        setDraft(profiles.find((profile) => profile.id !== draft.id) ?? draft);
                                    }}>
                                    <IconTrash size={11} /> delete
                                </button>
                            )}
                            <button
                                className="settings-btn primary"
                                type="button"
                                disabled={!draft.name.trim()}
                                onClick={() => cmd.saveProviderProfile({ ...draft, name: draft.name.trim() })}>
                                <IconSave size={11} /> {isSaved ? "save profile" : "add profile"}
                            </button>
                        </div>
                    </div>
                </div>
                <div className="provider-defaults">
                    {(["claude", "codex"] as const).map((type) => {
                        const options = profiles.filter((profile) => profile.provider === type);
                        return (
                            <label key={type}>
                                <span>{type} default</span>
                                <Dropdown
                                    className="settings-dd"
                                    label={`${type} default`}
                                    value={selectedProfiles[type] ?? ""}
                                    options={options.map((profile) => ({ value: profile.id, label: profile.name }))}
                                    onChange={(value) => cmd.selectProviderProfile(type, value)}
                                />
                            </label>
                        );
                    })}
                </div>
            </SettingsSection>
            <SettingsSection id="agents.restart">
                <ToggleSetting
                    label="Restore agent tabs"
                    detail="Bring resumable tabs back asleep. They start only when you select them."
                    checked={restore}
                    onChange={cmd.setRestoreAgentTabs}
                />
                <div className="command-editor-actions">
                    <button className="settings-btn" type="button" onClick={cmd.sleepIdleAgents}>
                        Sleep idle agents now
                    </button>
                </div>
            </SettingsSection>
            <SettingsSection id="agents.density" meta={density}>
                <Dropdown
                    className="settings-dd"
                    label="rail density"
                    value={density}
                    options={[
                        { value: "comfortable", label: "comfortable", detail: "Full labels and generous rows" },
                        { value: "compact", label: "compact", detail: "More sessions per screen" },
                    ]}
                    onChange={(value) => cmd.setRailDensity(value as "comfortable" | "compact")}
                />
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
        <SettingsPage page="cli">
            <SettingsSection id="cli.integration" meta={stateLabel}>
                <p className="settings-field-help">{status?.message ?? "Checking the packaged command-line integration…"}</p>
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
            <SettingsSection id="cli.usage">
                <pre className="cli-usage">{`sikemux .\nsikemux src/App.tsx:42:5\nsikemux open --wait README.md\nEDITOR=sikemux-editor git commit`}</pre>
            </SettingsSection>
        </SettingsPage>
    );
}

function AboutPage() {
    const updateChannel = useStore((s) => s.updateChannel);
    const lastUpdateCheck = useStore((s) => s.lastUpdateCheck);
    const pendingUpdate = useStore((s) => s.pendingUpdate);
    return (
        <SettingsPage page="about">
            <SettingsSection id="about.channel" meta={updateChannel}>
                <Dropdown
                    className="settings-dd"
                    label="update channel"
                    value={updateChannel}
                    options={[
                        { value: "stable", label: "stable", detail: "Latest signed release" },
                        { value: "preview", label: "preview", detail: "Signed moving preview release" },
                    ]}
                    onChange={(value) => cmd.setUpdateChannel(value as "stable" | "preview")}
                />
                <div className="about-actions">
                    <button className="settings-btn" disabled={isUpdateBusy(pendingUpdate?.state)} onClick={() => void cmd.checkForUpdates()}>
                        Check for updates
                    </button>
                </div>
                {lastUpdateCheck && <p className="settings-field-help">{updateCheckLabel(lastUpdateCheck)}</p>}
            </SettingsSection>
            <SettingsSection id="about.support">
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
            <SettingsSection id="about.transfer">
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
    projectRoots: ProjectRoot[];
    home: string;
    pretty: (p: string) => string;
}

function GeneralPage({ projectRoots, home, pretty }: GeneralPageProps) {
    const [draftPath, setDraftPath] = useState("");
    const [draftDepth, setDraftDepth] = useState(1);
    const [draftSelfIndex, setDraftSelfIndex] = useState(false);

    const resolveDirectory = async (raw: string) => {
        const expanded = await settingsApi.expandPath(raw);
        const ok = await settingsApi.isDirectory(expanded);
        if (!ok) {
            notify("error", `settings: not a directory: ${pretty(expanded)}`);
            return null;
        }
        return expanded;
    };

    const commitDraft = async () => {
        const raw = draftPath.trim();
        if (!raw) return;
        try {
            const expanded = await resolveDirectory(raw);
            if (!expanded) return;
            cmd.addProjectRoot(expanded, draftDepth, draftSelfIndex);
            setDraftPath("");
            setDraftDepth(1);
            setDraftSelfIndex(false);
        } catch (err) {
            reportError("settings")(err);
        }
    };

    const onPick = async () => {
        try {
            const picked = await settingsApi.pickFolder(home || undefined);
            if (picked) cmd.addProjectRoot(picked, draftDepth, draftSelfIndex);
        } catch (err) {
            reportError("folder picker")(err);
        }
    };

    return (
        <SettingsPage page="general">
            <SettingsSection id="general.roots" meta={`${projectRoots.length} ${projectRoots.length === 1 ? "folder" : "folders"}`}>
                <div className="settings-add">
                    <input
                        className="settings-input"
                        placeholder="~/proj    or    /Users/me/work"
                        value={draftPath}
                        onChange={(e) => setDraftPath(e.target.value)}
                        onKeyDown={(e) => {
                            if (e.key === "Enter") {
                                e.preventDefault();
                                void commitDraft();
                            } else if (e.key === "Escape") {
                                cmd.closeSettings();
                            }
                        }}
                        spellCheck={false}
                    />
                    <DepthStepper value={draftDepth} onChange={setDraftDepth} title="Levels to scan" />
                    <Tooltip label="Browse…">
                        <button className="settings-btn" onClick={onPick} type="button" aria-label="Browse for a folder">
                            <IconFolder size={11} />
                        </button>
                    </Tooltip>
                    <button className="settings-btn primary" onClick={() => void commitDraft()} disabled={!draftPath.trim()} type="button">
                        <IconPlus size={11} /> Add
                    </button>
                </div>
                <Checkbox checked={draftSelfIndex} onChange={setDraftSelfIndex}>
                    Index the folder itself as a project
                </Checkbox>

                {projectRoots.length === 0 ? (
                    <EmptyState
                        icon={<IconFolder size={14} />}
                        title="No project folders"
                        message="Add the folder your repositories live in, or open one from the session picker."
                    />
                ) : (
                    <div className="settings-list">
                        {projectRoots.map((root) => (
                            <div className="settings-list-row" key={root.path}>
                                <span className="settings-list-path">{pretty(root.path)}</span>
                                <Checkbox checked={root.selfIndex === true} onChange={(on) => cmd.setProjectRootSelfIndex(root.path, on)}>
                                    index itself
                                </Checkbox>
                                <DepthStepper
                                    value={root.depth}
                                    onChange={(depth) => cmd.setProjectRootDepth(root.path, depth)}
                                    title={`Levels scanned under ${pretty(root.path)}`}
                                />
                                <Tooltip label="Remove">
                                    <button
                                        className="settings-row-x"
                                        onClick={() => cmd.removeProjectRoot(root.path)}
                                        aria-label={`Remove ${pretty(root.path)}`}
                                        type="button">
                                        <IconClose size={11} />
                                    </button>
                                </Tooltip>
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
        <SettingsPage page="keybindings">
            <SettingsSection id="keybindings.map" meta={`${KEYBINDING_ACTIONS.length} commands · ${overrideCount} changed`}>
                <div className="keymap-toolbar">
                    <label className="keymap-search">
                        <span>filter commands</span>
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
    /** Themes matching the requested appearance, plus the current pick so it stays selectable. */
    const themeOptions = (dark: boolean, selectedId: string): DropdownOption[] =>
        [...THEMES, ...customThemes]
            .filter((theme) => theme.dark === dark || theme.id === selectedId)
            .map((theme) => ({
                value: theme.id,
                label: theme.name,
                ...(customThemes.some((candidate) => candidate.id === theme.id) ? { detail: "custom" } : {}),
            }));
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
        <SettingsPage page="appearance">
            <SettingsSection id="appearance.host" meta={themeMode}>
                <ToggleSetting
                    label="Follow system light/dark"
                    detail="Switches immediately when the host appearance changes."
                    checked={themeMode === "system"}
                    onChange={(enabled) => cmd.setThemeMode(enabled ? "system" : "manual")}
                />
                <div className="system-theme-grid">
                    <label>
                        <span>Light appearance</span>
                        <Dropdown
                            className="settings-dd"
                            label="Light appearance"
                            value={systemLightThemeId}
                            options={themeOptions(false, systemLightThemeId)}
                            onChange={cmd.setSystemLightThemeId}
                        />
                    </label>
                    <label>
                        <span>Dark appearance</span>
                        <Dropdown
                            className="settings-dd"
                            label="Dark appearance"
                            value={systemDarkThemeId}
                            options={themeOptions(true, systemDarkThemeId)}
                            onChange={cmd.setSystemDarkThemeId}
                        />
                    </label>
                </div>
            </SettingsSection>
            <SettingsSection id="appearance.theme" meta={`${THEMES.length} built-in · ${customThemes.length} custom`}>
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
                <SettingsSection id="appearance.window">
                    <div className="settings-control-stack">
                        <div className="settings-control">
                            <div className="settings-control-copy">
                                <h3>Opacity</h3>
                                <p>Solid at 1.00, translucent below it.</p>
                            </div>
                            <div className="settings-knob-row">
                                <Slider label="Window opacity" min={0} max={1} step={0.01} value={windowOpacity} onChange={cmd.setWindowOpacity} />
                                <NumberField value={windowOpacity} onCommit={cmd.setWindowOpacity} format={(v) => v.toFixed(2)} suffix="opacity" />
                            </div>
                        </div>
                        <div className="settings-control">
                            <div className="settings-control-copy">
                                <h3>Background blur</h3>
                                <p>0 is crisp; 20–40px gives a soft frosted effect.</p>
                            </div>
                            <div className="settings-knob-row">
                                <Slider
                                    label="Background blur"
                                    min={0}
                                    max={60}
                                    step={1}
                                    value={Math.min(60, windowBlur)}
                                    onChange={(value) => cmd.setWindowBlur(Math.round(value))}
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
        <SettingsPage page="cloud">
            <SettingsSection id="cloud.browser" meta="aws · gcp · sso">
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

            <SettingsSection id="cloud.workspace" meta="optional">
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

function SettingsPage({ page, children }: { page: SettingsPageId; children: ReactNode }) {
    const { query } = useSettingsSearch();
    return (
        <div className="settings-page">
            {!query && (
                <header className="settings-page-head">
                    <p className="settings-page-deck">{settingsPage(page).deck}</p>
                </header>
            )}
            {children}
        </div>
    );
}

function SettingsSection({ id, meta, children }: { id: SettingsSectionId; meta?: ReactNode; children: ReactNode }) {
    const { matches } = useSettingsSearch();
    const { title, sub } = settingsSection(id);
    if (!matches.sections.has(id)) return null;
    return (
        <section className="settings-section">
            <div className="settings-section-head">
                <h2 className="settings-section-title">{title}</h2>
                {meta && <span className="settings-section-meta">{meta}</span>}
            </div>
            <p className="settings-section-sub">{sub}</p>
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
            <span className="settings-depth-label">depth</span>
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
