import { useModalFocus } from "../hooks/useModalFocus";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent, ReactNode } from "react";
import { invokeCommand as invoke } from "../api/invoke";
import {
    eventToKeybinding,
    findKeybindingConflict,
    keybindingActions,
    type KeybindingAction,
    keybindingCategories,
    keybindingHasModifier,
    keybindingLabel,
    resolvedKeybinding,
    type KeybindingActionId,
    type KeybindingOverrides,
} from "../keybindings";
import { settingsApi } from "../api/settings";
import { isUpdateBusy, updateCheckLabel } from "../api/updater";
import { prettyPath } from "../lib/paths";
import { IS_MACOS, PRIMARY_SHORTCUT } from "../lib/platform";
import { notify, reportError } from "../state/toast";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { cloneTheme, newCustomThemeId, THEME_GROUPS, THEMES, themeFromColours, type Theme, type ThemeGroupKey } from "../themes";
import { wallpaperPixels, wallpaperTheme } from "../themes/wallpaper";
import { ThemePicker } from "./ThemePicker";
import {
    IconActivity,
    IconAgent,
    IconCheck,
    IconClose,
    IconCommand,
    IconContrast,
    IconEditor,
    IconFolder,
    IconGlobe,
    IconPlug,
    IconInfo,
    IconPlus,
    IconRefresh,
    IconRun,
    IconSave,
    IconSearch,
    IconTrash,
} from "./Icons";
import { Dropdown } from "./Dropdown";
import { Checkbox, Slider, Switch } from "./Controls";
import { Tooltip } from "./Tooltip";
import type { CommandContext, CustomCommand, CustomCommandPlacement } from "../commands/registry";
import type { AgentProvider, ProjectRoot, ProviderProfile } from "../state/types";
import { AGENT_PERMISSION_COPY, AGENT_PERMISSION_MODES } from "../agentLaunch";
import {
    searchSettings,
    SETTINGS_GROUPS,
    SETTINGS_INDEX,
    SETTINGS_PAGE_NAMES,
    SETTINGS_PAGE_ORDER,
    type SettingsEntry,
    type SettingsPageId,
} from "../settingsIndex";
import { useBuiltPlugins } from "../plugins/enabled";
import { frontendPlugin, pluginSurface } from "../plugins/registry";
import { ActivityPage } from "./ActivityPage";
import { SettingsPage, SettingsSection } from "./SettingsLayout";
import "../styles/settings.css";

const PAGE_ICONS: Record<SettingsPageId, ReactNode> = {
    general: <IconFolder size={13} />,
    appearance: <IconContrast size={13} />,
    keybindings: <IconCommand size={13} />,
    activity: <IconActivity size={13} />,
    about: <IconInfo size={13} />,
    agents: <IconAgent size={13} />,
    actions: <IconRun size={13} />,
    cli: <IconEditor size={13} />,
    cloud: <IconGlobe size={13} />,
    plugins: <IconPlug size={13} />,
};

const FOCUSABLE = "button:not(:disabled), input:not(:disabled), textarea:not(:disabled), select:not(:disabled), [tabindex]:not([tabindex='-1'])";

const RAIL_STEPS: Record<string, (index: number) => number> = {
    ArrowDown: (index) => (index + 1) % SETTINGS_PAGE_ORDER.length,
    ArrowRight: (index) => (index + 1) % SETTINGS_PAGE_ORDER.length,
    ArrowUp: (index) => (index - 1 + SETTINGS_PAGE_ORDER.length) % SETTINGS_PAGE_ORDER.length,
    ArrowLeft: (index) => (index - 1 + SETTINGS_PAGE_ORDER.length) % SETTINGS_PAGE_ORDER.length,
    Home: () => 0,
    End: () => SETTINGS_PAGE_ORDER.length - 1,
};

function isFindShortcut(event: KeyboardEvent): boolean {
    const primary = IS_MACOS ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
    return primary && !event.shiftKey && !event.altKey && event.code === "KeyF";
}

export function SettingsPanel() {
    const modalRef = useRef<HTMLDivElement>(null);
    useModalFocus(modalRef);
    const projectRoots = useStore((s) => s.projectRoots);
    const themeId = useStore((s) => s.themeId);
    const windowOpacity = useStore((s) => s.windowOpacity);
    const windowBlur = useStore((s) => s.windowBlur);
    const cloudBrowser = useStore((s) => s.cloudBrowser);
    const cloudBrowserShortcut = useStore((s) => s.cloudBrowserShortcut);
    const keybindingOverrides = useStore((s) => s.keybindingOverrides);
    const home = useStore((s) => s.home);
    const page = useStore((s) => s.settingsPage);
    const settingsBinding = resolvedKeybinding(keybindingOverrides, "settings.toggle");
    const closeSettingsHint = settingsBinding ? `Esc / ${keybindingLabel(settingsBinding)}` : "Esc";

    const [query, setQuery] = useState("");
    const [activeResult, setActiveResult] = useState(0);
    const [jump, setJump] = useState<{ entry: SettingsEntry; at: number } | null>(null);
    const searchRef = useRef<HTMLInputElement>(null);
    const scrollRef = useRef<HTMLDivElement>(null);
    const railItems = useRef(new Map<SettingsPageId, HTMLButtonElement>());

    const entries = useMemo<SettingsEntry[]>(
        () => [
            ...SETTINGS_INDEX,
            ...keybindingActions().map((action) => ({
                page: "keybindings" as const,
                section: "Shortcuts",
                label: action.label,
                target: "Shortcuts",
                keywords: `${action.detail} shortcut ${keybindingLabel(resolvedKeybinding(keybindingOverrides, action.id as KeybindingActionId))}`,
                filter: action.label,
            })),
        ],
        [keybindingOverrides],
    );
    const results = useMemo(() => searchSettings(query, entries), [query, entries]);
    const searching = query.trim().length > 0;

    useEffect(() => {
        searchRef.current?.focus();
    }, []);

    useEffect(() => {
        const onKey = (e: KeyboardEvent) => {
            if (e.key === "Escape") {
                e.preventDefault();
                cmd.closeSettings();
            } else if (isFindShortcut(e)) {
                e.preventDefault();
                searchRef.current?.focus();
                searchRef.current?.select();
            }
        };
        window.addEventListener("keydown", onKey);
        return () => window.removeEventListener("keydown", onKey);
    }, []);

    useEffect(() => {
        if (!jump) return;
        const target = [...(scrollRef.current?.querySelectorAll<HTMLElement>("[data-settings-target]") ?? [])].find(
            (element) => element.dataset.settingsTarget === jump.entry.target,
        );
        if (!target) return;
        target.scrollIntoView({ block: "center", behavior: "smooth" });
        target.querySelector<HTMLElement>(FOCUSABLE)?.focus({ preventScroll: true });
        delete target.dataset.settingsFlash;
        void target.offsetWidth;
        target.dataset.settingsFlash = "";
        const timer = window.setTimeout(() => delete target.dataset.settingsFlash, 1600);
        return () => window.clearTimeout(timer);
    }, [jump]);

    const goTo = (next: SettingsPageId) => {
        setJump(null);
        setQuery("");
        cmd.setSettingsPage(next);
    };

    const openEntry = (entry: SettingsEntry) => {
        setQuery("");
        setActiveResult(0);
        cmd.setSettingsPage(entry.page);
        setJump({ entry, at: Date.now() });
    };

    const onSearchKey = (e: ReactKeyboardEvent<HTMLInputElement>) => {
        if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            if (!results.length) return;
            e.preventDefault();
            setActiveResult((index) => (e.key === "ArrowDown" ? Math.min(index + 1, results.length - 1) : Math.max(index - 1, 0)));
        } else if (e.key === "Enter") {
            const entry = results[activeResult];
            if (!entry) return;
            e.preventDefault();
            openEntry(entry);
        } else if (e.key === "Escape" && query) {
            e.preventDefault();
            e.stopPropagation();
            setQuery("");
        }
    };

    const onRailKey = (e: ReactKeyboardEvent<HTMLElement>) => {
        const step = RAIL_STEPS[e.key];
        if (!step || !(e.target instanceof HTMLElement) || !e.target.closest(".settings-rail-item")) return;
        e.preventDefault();
        const next = SETTINGS_PAGE_ORDER[step(SETTINGS_PAGE_ORDER.indexOf(page))];
        goTo(next);
        railItems.current.get(next)?.focus();
    };

    const pretty = (p: string) => prettyPath(p, home);

    return (
        <div ref={modalRef} tabIndex={-1} className="settings-pane" role="dialog" aria-modal="true" aria-label="Settings">
            <div className="settings-frame">
                <aside className="settings-rail">
                    <label className="settings-search">
                        <IconSearch size={12} />
                        <input
                            ref={searchRef}
                            value={query}
                            onChange={(e) => {
                                setQuery(e.target.value);
                                setActiveResult(0);
                            }}
                            onKeyDown={onSearchKey}
                            placeholder="Search settings"
                            aria-label="Search settings"
                            role="combobox"
                            aria-autocomplete="list"
                            aria-expanded={searching}
                            aria-controls="settings-results"
                            aria-activedescendant={searching && results.length ? `settings-result-${activeResult}` : undefined}
                            spellCheck={false}
                        />
                        {!query && <kbd className="settings-search-key">{PRIMARY_SHORTCUT}F</kbd>}
                    </label>

                    <nav className="settings-nav" aria-label="Settings sections" onKeyDown={onRailKey}>
                        {SETTINGS_GROUPS.map((group) => (
                            <div className="settings-nav-group" key={group.label}>
                                <span className="settings-nav-label">{group.label}</span>
                                {group.pages.map((id) => (
                                    <button
                                        key={id}
                                        ref={(node) => {
                                            if (node) railItems.current.set(id, node);
                                            else railItems.current.delete(id);
                                        }}
                                        className={`settings-rail-item${page === id && !searching ? " active" : ""}`}
                                        onClick={() => goTo(id)}
                                        aria-current={page === id ? "page" : undefined}
                                        type="button">
                                        <span className="settings-rail-icon" aria-hidden="true">
                                            {PAGE_ICONS[id]}
                                        </span>
                                        <span className="settings-rail-name">{SETTINGS_PAGE_NAMES[id]}</span>
                                    </button>
                                ))}
                            </div>
                        ))}
                    </nav>

                    <div className="settings-rail-foot">Changes save automatically</div>
                </aside>

                <div className="settings-main">
                    <header className="settings-topbar">
                        <span className="settings-topbar-title">{searching ? "Search" : SETTINGS_PAGE_NAMES[page]}</span>
                        <button
                            className="settings-topbar-close"
                            onClick={cmd.closeSettings}
                            title={`Close settings (${closeSettingsHint})`}
                            aria-label="Close settings"
                            type="button">
                            <IconClose size={14} />
                        </button>
                    </header>

                    <div className="settings-scroll" ref={scrollRef}>
                        {searching ? (
                            <SearchResults query={query} results={results} active={activeResult} onHover={setActiveResult} onOpen={openEntry} />
                        ) : (
                            <>
                                {page === "general" && <GeneralPage projectRoots={projectRoots} home={home} pretty={pretty} />}

                                {page === "appearance" && <AppearancePage themeId={themeId} windowOpacity={windowOpacity} windowBlur={windowBlur} />}

                                {page === "keybindings" && (
                                    <KeybindingsPage key={jump?.at} overrides={keybindingOverrides} initialQuery={jump?.entry.filter ?? ""} />
                                )}

                                {page === "activity" && <ActivityPage />}

                                {page === "about" && <AboutPage />}

                                {page === "agents" && <AgentsPage />}

                                {page === "actions" && <ActionsPage />}

                                {page === "cli" && <CliPage />}

                                {page === "cloud" && <CloudPage cloudBrowser={cloudBrowser} cloudBrowserShortcut={cloudBrowserShortcut} />}
                                {page === "plugins" && <PluginsPage />}
                            </>
                        )}
                    </div>
                </div>
            </div>
        </div>
    );
}

interface SearchResultsProps {
    query: string;
    results: SettingsEntry[];
    active: number;
    onHover: (index: number) => void;
    onOpen: (entry: SettingsEntry) => void;
}

function SearchResults({ query, results, active, onHover, onOpen }: SearchResultsProps) {
    return (
        <SettingsPage>
            {results.length === 0 ? (
                <div className="settings-empty">No settings match “{query.trim()}”.</div>
            ) : (
                <div className="settings-results" id="settings-results" role="listbox" aria-label="Matching settings">
                    {results.map((entry, index) => {
                        const pageName = SETTINGS_PAGE_NAMES[entry.page];
                        const path = entry.section === entry.label ? pageName : `${pageName} › ${entry.section}`;
                        return (
                            <button
                                key={`${entry.page}:${entry.section}:${entry.label}`}
                                id={`settings-result-${index}`}
                                className={`settings-result${index === active ? " active" : ""}`}
                                role="option"
                                aria-selected={index === active}
                                tabIndex={-1}
                                type="button"
                                onMouseMove={() => index !== active && onHover(index)}
                                onClick={() => onOpen(entry)}>
                                <span className="settings-rail-icon" aria-hidden="true">
                                    {PAGE_ICONS[entry.page]}
                                </span>
                                <span className="settings-result-label">{entry.label}</span>
                                <span className="settings-result-path">{path}</span>
                            </button>
                        );
                    })}
                </div>
            )}
        </SettingsPage>
    );
}

const CORE_COMMAND_CONTEXTS: readonly CommandContext[] = ["project", "command", "ssh"];
const COMMAND_PLACEMENTS: CustomCommandPlacement[] = ["terminal", "split", "popup", "background", "replace"];

function blankCommand(): CustomCommand {
    return { id: `command-${Date.now().toString(36)}`, title: "", detail: "", command: "", contexts: [], placement: "terminal" };
}

function ActionsPage() {
    const commands = useStore((s) => s.customCommands);
    const pluginManifests = useStore((s) => s.pluginManifests);
    const contextOptions = useMemo(
        () => [
            ...CORE_COMMAND_CONTEXTS,
            ...pluginManifests.flatMap((manifest) => frontendPlugin(manifest.id)?.surfaces.map((surface) => surface.kind) ?? []),
        ],
        [pluginManifests],
    );
    const [draft, setDraft] = useState<CustomCommand>(() => blankCommand());
    const editing = commands.some((item) => item.id === draft.id);
    const save = () => {
        if (!draft.title.trim() || !draft.command.trim()) return;
        cmd.upsertCustomCommand({ ...draft, title: draft.title.trim(), detail: draft.detail.trim() });
        setDraft(blankCommand());
    };
    return (
        <SettingsPage>
            <SettingsSection title="Your actions" meta={`${commands.length} saved`} sub="Shell commands that sit beside the built-in ones.">
                {commands.length === 0 ? (
                    <div className="settings-empty">No custom actions yet. The built-ins are already searchable from the command deck.</div>
                ) : (
                    <div className="custom-command-list">
                        {commands.map((item) => (
                            <button key={item.id} type="button" onClick={() => setDraft(item)}>
                                <span>{item.title}</span>
                                <small>
                                    {item.placement} · {item.contexts.length ? item.contexts.join(", ") : "all contexts"}
                                </small>
                            </button>
                        ))}
                    </div>
                )}
            </SettingsSection>

            <SettingsSection title={editing ? "Edit action" : "New action"}>
                <SettingsRows>
                    <SettingsRow label="Name" wide>
                        <input
                            className="settings-input wide"
                            aria-label="Display name"
                            placeholder="Display name"
                            value={draft.title}
                            onChange={(e) => setDraft({ ...draft, title: e.target.value })}
                        />
                    </SettingsRow>
                    <SettingsRow label="Description" desc="Shown under the name in the deck." wide>
                        <input
                            className="settings-input wide"
                            aria-label="What it does"
                            placeholder="What it does"
                            value={draft.detail}
                            onChange={(e) => setDraft({ ...draft, detail: e.target.value })}
                        />
                    </SettingsRow>
                    <SettingsRow
                        label="Command"
                        desc="Runs unsandboxed in the active session's directory, with SIKEMUX_SESSION_* and SIKEMUX_PROJECT set."
                        stack>
                        <textarea
                            className="settings-input mono command-editor-source"
                            aria-label="Shell command"
                            placeholder="shell command"
                            value={draft.command}
                            onChange={(e) => setDraft({ ...draft, command: e.target.value })}
                            spellCheck={false}
                        />
                    </SettingsRow>
                    <SettingsRow label="Where output lands" desc="A terminal tab, a split, a popup, a background toast, or this pane." wide>
                        <Dropdown
                            className="settings-dd"
                            label="placement"
                            value={draft.placement}
                            options={COMMAND_PLACEMENTS.map((value) => ({ value, label: value[0].toUpperCase() + value.slice(1) }))}
                            onChange={(value) => setDraft({ ...draft, placement: value as CustomCommandPlacement })}
                        />
                    </SettingsRow>
                    <SettingsRow label="Contexts" desc="Leave all unticked to offer it everywhere." stack>
                        <div className="command-contexts">
                            {contextOptions.map((context) => (
                                <Checkbox
                                    key={context}
                                    checked={draft.contexts.includes(context)}
                                    onChange={(on) =>
                                        setDraft({
                                            ...draft,
                                            contexts: on ? [...draft.contexts, context] : draft.contexts.filter((item) => item !== context),
                                        })
                                    }>
                                    {pluginSurface(context)?.title ?? context}
                                </Checkbox>
                            ))}
                        </div>
                    </SettingsRow>
                </SettingsRows>

                <div className="settings-actions">
                    <button className="settings-btn" type="button" onClick={() => setDraft(blankCommand())}>
                        New
                    </button>
                    {editing && (
                        <button
                            className="settings-btn danger"
                            type="button"
                            onClick={() => {
                                cmd.deleteCustomCommand(draft.id);
                                setDraft(blankCommand());
                            }}>
                            <IconTrash size={12} /> Delete
                        </button>
                    )}
                    <button className="settings-btn primary" type="button" disabled={!draft.title.trim() || !draft.command.trim()} onClick={save}>
                        <IconSave size={12} /> Save
                    </button>
                </div>
            </SettingsSection>
        </SettingsPage>
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
        <SettingsPage>
            <SettingsSection
                title="Launch boundary"
                meta={AGENT_PERMISSION_COPY[defaultPermissionMode].label}
                sub="Offered before every launch. Providers without a matching CLI control fall back to their own settings.">
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

            <SettingsSection
                title="Provider profiles"
                meta={`${profiles.length} configured`}
                sub="Which local executable a launch uses. Credentials are never saved by Sikemux.">
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
                            <IconPlus size={12} /> New profile
                        </button>
                    </div>
                    <div className="provider-profile-editor">
                        <SettingsRows>
                            <SettingsRow label="Name" wide>
                                <input
                                    className="settings-input wide"
                                    aria-label="name"
                                    value={draft.name}
                                    onChange={(event) => setDraft({ ...draft, name: event.target.value })}
                                />
                            </SettingsRow>
                            <SettingsRow label="Provider" wide>
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
                            </SettingsRow>
                            <SettingsRow label="Executable path" desc="Empty uses PATH." wide>
                                <input
                                    className="settings-input mono wide"
                                    aria-label="executable path"
                                    placeholder="Leave empty to use PATH"
                                    value={draft.executablePath ?? ""}
                                    onChange={(event) => setDraft({ ...draft, executablePath: event.target.value || undefined })}
                                />
                            </SettingsRow>
                            {(draft.provider === "claude" || draft.provider === "codex") && (
                                <SettingsRow
                                    label="Profile directory"
                                    desc={`Set this only when keeping several ${draft.provider} accounts side by side.`}
                                    wide>
                                    <input
                                        className="settings-input mono wide"
                                        aria-label="profile directory"
                                        placeholder={draft.provider === "codex" ? "Automatic, or ~/.codex-work" : "Automatic, or ~/.claude-work"}
                                        value={draft.configPath ?? ""}
                                        onChange={(event) => setDraft({ ...draft, configPath: event.target.value || undefined })}
                                    />
                                </SettingsRow>
                            )}
                        </SettingsRows>
                        <div className="settings-actions">
                            {isSaved && !draft.id.startsWith("builtin-") && (
                                <button
                                    className="settings-btn danger"
                                    type="button"
                                    onClick={() => {
                                        cmd.deleteProviderProfile(draft.id);
                                        setDraft(profiles.find((profile) => profile.id !== draft.id) ?? draft);
                                    }}>
                                    <IconTrash size={12} /> Delete
                                </button>
                            )}
                            <button
                                className="settings-btn primary"
                                type="button"
                                disabled={!draft.name.trim()}
                                onClick={() => cmd.saveProviderProfile({ ...draft, name: draft.name.trim() })}>
                                <IconSave size={12} /> {isSaved ? "Save profile" : "Add profile"}
                            </button>
                        </div>
                    </div>
                </div>

                <SettingsRows>
                    {(["claude", "codex"] as const).map((type) => (
                        <SettingsRow
                            key={type}
                            label={`${type[0].toUpperCase()}${type.slice(1)} default`}
                            desc={`The profile a new ${type} session launches with.`}
                            wide>
                            <Dropdown
                                className="settings-dd"
                                label={`${type} default`}
                                value={selectedProfiles[type] ?? ""}
                                options={profiles
                                    .filter((profile) => profile.provider === type)
                                    .map((profile) => ({
                                        value: profile.id,
                                        label: profile.name,
                                    }))}
                                onChange={(value) => cmd.selectProviderProfile(type, value)}
                            />
                        </SettingsRow>
                    ))}
                </SettingsRows>
            </SettingsSection>

            <SettingsSection title="Sessions">
                <SettingsRows>
                    <SettingsRow
                        label="Restore agent tabs"
                        desc="Resumable tabs come back asleep and start only when selected."
                        asLabel
                        control={<Switch checked={restore} onChange={cmd.setRestoreAgentTabs} label="Restore agent tabs" />}
                    />
                    <SettingsRow label="Rail density" desc="Compact fits more sessions while keeping every state symbol visible." wide>
                        <Dropdown
                            className="settings-dd"
                            label="rail density"
                            value={density}
                            options={[
                                { value: "comfortable", label: "Comfortable", detail: "Full labels and generous rows" },
                                { value: "compact", label: "Compact", detail: "More sessions per screen" },
                            ]}
                            onChange={(value) => cmd.setRailDensity(value as "comfortable" | "compact")}
                        />
                    </SettingsRow>
                    <SettingsRow label="Idle agents" desc="Put every idle agent to sleep now, freeing its process.">
                        <button className="settings-btn" type="button" onClick={() => void cmd.sleepIdleAgents()}>
                            Sleep now
                        </button>
                    </SettingsRow>
                </SettingsRows>
                <p className="settings-hint">
                    Only confirmed native session IDs are written to disk. Startup commands and terminal output never are.
                </p>
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
        <SettingsPage>
            <SettingsSection title="Shell integration" meta={stateLabel} sub={status?.message ?? "Checking the packaged command-line integration…"}>
                <div className="cli-paths">
                    <span>
                        <b>Commands</b>
                        <code>sikemux · sikemux-editor</code>
                    </span>
                    <span>
                        <b>Install directory</b>
                        <code>{status?.installDir || "—"}</code>
                    </span>
                </div>
                <div className="settings-actions">
                    <button className="settings-btn" type="button" disabled={busy} onClick={refresh}>
                        <IconRefresh size={12} /> Refresh
                    </button>
                    <button className="settings-btn primary" type="button" disabled={installDisabled} onClick={() => void install()}>
                        {status?.state === "installed" && <IconCheck size={12} />}
                        {buttonLabel}
                    </button>
                </div>
                {status?.state === "conflict" && (
                    <p className="settings-hint danger">
                        Sikemux will not overwrite <em>{status.cliPath}</em> or <em>{status.editorPath}</em>. Move the existing file yourself, then
                        refresh.
                    </p>
                )}
                {status?.state === "installed" && !status.pathConfigured && (
                    <p className="settings-hint">
                        Add <em>{status.installDir}</em> to your shell’s PATH. Sikemux never edits shell startup files.
                    </p>
                )}
            </SettingsSection>

            <SettingsSection title="Usage" sub="Existing files open in an editor tab. Directories focus or create their workspace.">
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
        <SettingsPage>
            <SettingsSection title="Updates">
                <SettingsRows>
                    <SettingsRow
                        label="Channel"
                        desc="Stable follows the latest signed release; nightly the newest build, prerelease or stable."
                        wide>
                        <Dropdown
                            className="settings-dd"
                            label="update channel"
                            value={updateChannel}
                            options={[
                                { value: "stable", label: "Stable", detail: "Latest signed release" },
                                { value: "nightly", label: "Nightly", detail: "Newest signed build, prerelease or stable" },
                            ]}
                            onChange={(value) => cmd.setUpdateChannel(value as "stable" | "nightly")}
                        />
                    </SettingsRow>
                    <SettingsRow label="Last checked" desc={lastUpdateCheck ? updateCheckLabel(lastUpdateCheck) : "Not checked yet this session."}>
                        <button className="settings-btn" disabled={isUpdateBusy(pendingUpdate?.state)} onClick={() => void cmd.checkForUpdates()}>
                            Check now
                        </button>
                    </SettingsRow>
                </SettingsRows>
            </SettingsSection>

            <SettingsSection title="Help" sub="All three are also searchable from the command deck.">
                <div className="settings-actions start">
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
        <SettingsPage>
            <SettingsSection
                title="Project folders"
                meta={`${projectRoots.length} ${projectRoots.length === 1 ? "folder" : "folders"}`}
                sub="Each folder is scanned for git repos, as deep as its depth allows.">
                <div className="settings-list">
                    {projectRoots.length > 0 && (
                        <>
                            <div className="settings-list-head">
                                <span>Folder</span>
                                <span>Index itself</span>
                                <span>Depth</span>
                                <span />
                            </div>
                            {projectRoots.map((root) => (
                                <div className="settings-list-row" key={root.path}>
                                    <span className="settings-list-path">{pretty(root.path)}</span>
                                    <Checkbox
                                        label={`Index ${pretty(root.path)} itself`}
                                        checked={root.selfIndex === true}
                                        onChange={(on) => cmd.setProjectRootSelfIndex(root.path, on)}
                                    />
                                    <DepthStepper
                                        compact
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
                        </>
                    )}

                    <div className="settings-list-add">
                        <div className="settings-add">
                            <input
                                className="settings-input mono"
                                aria-label="Folder to add"
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
                                    <IconFolder size={12} />
                                </button>
                            </Tooltip>
                            <button className="settings-btn primary" onClick={() => void commitDraft()} disabled={!draftPath.trim()} type="button">
                                <IconPlus size={12} /> Add
                            </button>
                        </div>
                        <Checkbox checked={draftSelfIndex} onChange={setDraftSelfIndex}>
                            Index the folder itself as a project
                        </Checkbox>
                    </div>
                </div>

                <p className="settings-hint">
                    Indexing a folder itself offers it in the picker even when it is not a repo — useful for a scratch directory.
                </p>
            </SettingsSection>

            <SettingsSection title="Session transfer" sub="Move a workspace between machines through the clipboard.">
                <div className="settings-actions start">
                    <button className="settings-btn" onClick={() => void cmd.exportActiveSession().catch(reportError("session export"))}>
                        Copy active session
                    </button>
                    <button className="settings-btn" onClick={() => void cmd.importSessionFromClipboard().catch(reportError("session import"))}>
                        Import from clipboard
                    </button>
                </div>
                <p className="settings-hint">
                    A bundle leaves out Bruno secrets, drafts, terminal history, environment values and startup commands. Imported agents arrive
                    dormant.
                </p>
            </SettingsSection>
        </SettingsPage>
    );
}

function KeybindingsPage({ overrides, initialQuery }: { overrides: KeybindingOverrides; initialQuery: string }) {
    const [query, setQuery] = useState(initialQuery);
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
            setMessage(`${keybindingActions().find((action) => action.id === id)?.label} is now unassigned.`);
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
        setMessage(`${keybindingActions().find((action) => action.id === id)?.label} changed to ${keybindingLabel(binding)}.`);
    };

    const matches = (action: KeybindingAction) =>
        !normalizedQuery ||
        `${action.label} ${action.detail} ${keybindingLabel(resolvedKeybinding(overrides, action.id as KeybindingActionId))}`
            .toLowerCase()
            .includes(normalizedQuery);

    return (
        <SettingsPage>
            <SettingsSection
                title="Shortcuts"
                meta={`${keybindingActions().length} commands · ${overrideCount} changed`}
                sub="Select a keycap, then press a new combination. Conflicts are blocked.">
                <div className="keymap-toolbar">
                    <label className="keymap-search">
                        <IconSearch size={12} />
                        <input
                            value={query}
                            onChange={(event) => setQuery(event.target.value)}
                            placeholder="Filter by name, description or key"
                            aria-label="Filter shortcuts"
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
                        <IconRefresh size={12} /> Reset all
                    </button>
                </div>

                <div className={`keymap-status${recording ? " listening" : ""}`} aria-live="polite">
                    <span className="keymap-status-light" />
                    <span>{message || "Select any keycap to record a replacement."}</span>
                </div>

                <div className="keymap-groups">
                    {keybindingCategories().map((category) => {
                        const actions = keybindingActions().filter((action) => action.category === category && matches(action));
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
                                                            <IconRefresh size={11} />
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
                    {normalizedQuery && !keybindingActions().some(matches) && (
                        <div className="settings-empty">No commands match “{query.trim()}”.</div>
                    )}
                </div>

                <p className="settings-hint">
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
    const uiTextScale = useStore((state) => state.uiTextScale);
    const customThemes = useStore((s) => s.customThemes);
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

    const [readingWallpaper, setReadingWallpaper] = useState(false);
    const fromWallpaper = async () => {
        setReadingWallpaper(true);
        try {
            const wallpaper = await settingsApi.wallpaperImage();
            const theme = themeFromColours(wallpaperTheme(await wallpaperPixels(wallpaper.dataUrl), `${wallpaper.name} wallpaper`));
            openEditor({ theme: cloneTheme(theme, { id: newCustomThemeId() }), original: cloneTheme(theme), isNew: true, baseName: theme.name });
        } catch (error) {
            reportError("Theme from wallpaper")(error);
        } finally {
            setReadingWallpaper(false);
        }
    };

    const editCustom = (src: Theme) => openEditor({ theme: cloneTheme(src), original: cloneTheme(src), isNew: false, baseName: src.name });

    const closeEditor = () => {
        setEdit(null);
        cmd.cancelThemePreview();
    };

    const saveEditor = () => {
        if (!edit) return;
        cmd.saveCustomTheme({ ...edit.theme, name: edit.theme.name.trim() || "custom theme" });
        setEdit(null);
    };

    return (
        <SettingsPage>
            <SettingsSection
                title="Theme"
                meta={`${THEMES.length} built-in · ${customThemes.length} custom`}
                sub="Applies instantly to chrome, editor and terminal. Arrow keys in the search step through the list.">
                <ThemePicker
                    themeId={themeId}
                    customThemes={customThemes}
                    editingId={edit?.theme.id}
                    onCustomize={customizeFrom}
                    onEdit={editCustom}
                    onFromWallpaper={fromWallpaper}
                    readingWallpaper={readingWallpaper}
                />
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

            <SettingsSection title="Interface">
                <SettingsRows>
                    <SettingsRow label="Text size" desc="Scales labels and controls without loosening the layout." wide>
                        <Dropdown
                            className="settings-dd"
                            label="Interface text size"
                            value={String(uiTextScale)}
                            options={[
                                { value: "1", label: "100% · Default" },
                                { value: "1.1", label: "110% · Larger" },
                                { value: "1.25", label: "125% · Largest" },
                            ]}
                            onChange={(value) => cmd.setUiTextScale(Number(value))}
                        />
                    </SettingsRow>
                </SettingsRows>
            </SettingsSection>

            {IS_MACOS && (
                <SettingsSection title="Window">
                    <SettingsRows>
                        <SettingsRow label="Opacity" desc="Solid at 1.00, translucent below it.">
                            <div className="settings-knob">
                                <Slider label="Window opacity" min={0} max={1} step={0.01} value={windowOpacity} onChange={cmd.setWindowOpacity} />
                                <NumberField value={windowOpacity} onCommit={cmd.setWindowOpacity} format={(v) => v.toFixed(2)} suffix="opacity" />
                            </div>
                        </SettingsRow>
                        <SettingsRow label="Background blur" desc="0 is crisp; 20–40px gives a soft frosted effect.">
                            <div className="settings-knob">
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
                        </SettingsRow>
                    </SettingsRows>
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
    const [groupKey, setGroupKey] = useState<ThemeGroupKey>(THEME_GROUPS[0].key);
    const group = THEME_GROUPS.find((candidate) => candidate.key === groupKey) ?? THEME_GROUPS[0];
    return (
        <section className="theme-editor">
            <header className="theme-editor-head">
                <div className="theme-editor-title">
                    <input
                        className="theme-editor-name"
                        value={theme.name}
                        spellCheck={false}
                        placeholder="theme name"
                        aria-label="Theme name"
                        onChange={(e) => onName(e.target.value)}
                        autoFocus
                    />
                    <span className="theme-editor-base">based on {baseName}</span>
                </div>
                <div className="theme-editor-tools">
                    <button
                        className="settings-btn"
                        onClick={() => onDark(!theme.dark)}
                        type="button"
                        title="Editor light/dark hint — affects CodeMirror defaults">
                        {theme.dark ? "Dark" : "Light"}
                    </button>
                    <button className="settings-btn" onClick={onReset} type="button" title="Revert all colours to the source theme">
                        Reset
                    </button>
                    <button className="settings-btn" onClick={onCancel} type="button">
                        <IconClose size={12} /> Cancel
                    </button>
                    <button className="settings-btn primary" onClick={onSave} type="button">
                        {isNew ? <IconSave size={12} /> : <IconCheck size={12} />} {isNew ? "Save theme" : "Update"}
                    </button>
                </div>
            </header>

            <ThemePreview theme={theme} />

            <div className="theme-editor-tabs" role="tablist" aria-label="Colour groups">
                {THEME_GROUPS.map((candidate) => (
                    <button
                        key={candidate.key}
                        type="button"
                        role="tab"
                        aria-selected={candidate.key === groupKey}
                        className={`theme-editor-tab${candidate.key === groupKey ? " active" : ""}`}
                        onClick={() => setGroupKey(candidate.key)}>
                        {candidate.label}
                    </button>
                ))}
            </div>
            <p className="settings-hint">{group.hint}</p>

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

            <p className="settings-hint">
                Any CSS colour works in the text box — use <em>rgba(…)</em> for a translucent wash. The picker only sets hex.
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
                <input type="color" value={toHex(value)} onChange={(e) => onChange(e.target.value)} aria-label={label} />
            </label>
            <div className="theme-field-body">
                <span className="theme-field-label">{label}</span>
                <input
                    className="theme-field-hex"
                    value={value}
                    spellCheck={false}
                    aria-label={`${label} value`}
                    onChange={(e) => onChange(e.target.value)}
                />
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

function PluginsPage() {
    const built = useBuiltPlugins();
    const manifests = useStore((s) => s.pluginManifests);
    const disabled = useStore((s) => s.disabledPlugins);
    return (
        <SettingsPage>
            <SettingsSection
                title="Built-in plugins"
                sub="A plugin switched off leaves the rail, the top bar and agents' tools, and costs nothing until it is back on.">
                <SettingsRows>
                    {built.length === 0 && <div className="settings-empty">No plugins in this build.</div>}
                    {built.map((plugin) => {
                        const title = plugin.surfaces[0]?.title ?? plugin.id;
                        const version = manifests.find((manifest) => manifest.id === plugin.id)?.version;
                        return (
                            <SettingsRow
                                key={plugin.id}
                                label={title}
                                desc={`${plugin.id}${version ? ` · ${version}` : ""}`}
                                asLabel
                                control={
                                    <Switch
                                        checked={!disabled.includes(plugin.id)}
                                        onChange={(enabled) => cmd.setPluginEnabled(plugin.id, enabled)}
                                        label={title}
                                    />
                                }
                            />
                        );
                    })}
                </SettingsRows>
            </SettingsSection>
        </SettingsPage>
    );
}

function CloudPage({ cloudBrowser, cloudBrowserShortcut }: CloudPageProps) {
    return (
        <SettingsPage>
            <SettingsSection title="Single sign-on" sub="Where an AWS or GCP sign-in URL opens, and where to go once it does.">
                <SettingsRows>
                    <SettingsRow label="Browser app" desc="Must match a running app’s name. A trailing .app is fine." wide>
                        <input
                            className="settings-input wide"
                            aria-label="Browser app"
                            placeholder="Zen, Arc, Safari · empty = system default"
                            value={cloudBrowser}
                            onChange={(e) => cmd.setCloudBrowser(e.target.value)}
                            spellCheck={false}
                        />
                    </SettingsRow>
                    <SettingsRow label="Workspace shortcut" desc="Fired right after the link opens, to reach the desktop the browser lives on." wide>
                        <input
                            className="settings-input mono wide"
                            aria-label="Workspace shortcut"
                            placeholder="ctrl+3 · empty = no switch"
                            value={cloudBrowserShortcut}
                            onChange={(e) => cmd.setCloudBrowserShortcut(e.target.value)}
                            spellCheck={false}
                        />
                    </SettingsRow>
                </SettingsRows>
            </SettingsSection>
        </SettingsPage>
    );
}

function SettingsRows({ children }: { children: ReactNode }) {
    return <div className="settings-rows">{children}</div>;
}

/**
 * The shape every labelled setting takes: a name, an optional line of help, and
 * one control. Pass `asLabel` when the control is a switch or checkbox, so the
 * whole row is clickable; `wide` when the control should fill the right column.
 */
function SettingsRow({
    label,
    desc,
    wide = false,
    stack = false,
    asLabel = false,
    control,
    children,
}: {
    label: ReactNode;
    desc?: ReactNode;
    wide?: boolean;
    stack?: boolean;
    asLabel?: boolean;
    control?: ReactNode;
    children?: ReactNode;
}) {
    const Tag = asLabel ? "label" : "div";
    return (
        <Tag
            className={`settings-row${wide ? " wide" : ""}${stack ? " stack" : ""}`}
            data-settings-target={typeof label === "string" ? label : undefined}>
            <span className="settings-row-copy">
                <span className="settings-row-label">{label}</span>
                {desc && <span className="settings-row-desc">{desc}</span>}
            </span>
            <span className="settings-row-control">{control ?? children}</span>
        </Tag>
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
                aria-label={suffix}
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

function DepthStepper({
    value,
    onChange,
    title,
    compact = false,
}: {
    value: number;
    onChange: (v: number) => void;
    title?: string;
    compact?: boolean;
}) {
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
            {!compact && <span className="settings-depth-label">depth</span>}
            <button className="settings-depth-btn" onClick={() => bump(-1)} disabled={value <= 0} type="button" aria-label="Scan one level less">
                −
            </button>
            <input
                type="text"
                inputMode="numeric"
                className="settings-depth-input"
                value={draft}
                spellCheck={false}
                aria-label="Levels to scan"
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
            <button className="settings-depth-btn" onClick={() => bump(1)} type="button" aria-label="Scan one level more">
                +
            </button>
        </div>
    );
}
