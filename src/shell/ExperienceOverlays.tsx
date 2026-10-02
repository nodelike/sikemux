import { useEffect, useMemo, useRef, useState } from "react";
import { invokeCommand as invoke } from "../api/invoke";
import { uiActivity, type UiActivityReport } from "../lib/activity";
import { browserDiagnostics, exportDiagnosticsSnapshot, nativeDiagnostics } from "../lib/diagnostics";
import { useResourceEnabled } from "../state/resources";
import { agentCatalogR } from "../state/resources.defs";
import { useStore } from "../state/store";
import * as cmd from "../state/commands";
import { agentDetectionApi, type ManifestReport } from "../api/agentDetection";
import { selectedAgentRuntimeProfiles } from "../agents/agentProfiles";
import { actionForEvent, keybindingLabelForAction, type CoreKeybindingActionId } from "../commands/keybindings";
import { AgentIcon, IconAgent, IconCommand, IconFolder, IconSearch, Logo } from "../ui/Icons";
import { Kbd } from "../ui/Kbd";
import { ShaderField } from "../ui/ShaderField";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useOccludeNativeViews } from "../state/nativeViews";
import { copyText } from "../lib/clipboard";

interface IntegrationHealth {
    git: boolean;
}

/** Real first moves. Picking one closes the welcome and runs the action. */
const WELCOME_MOVES = [
    {
        id: "project.open",
        label: "Open a project",
        detail: "Pick a folder or repository to work in",
        Icon: IconFolder,
        run: () => cmd.openPicker("projects"),
    },
    {
        id: "agent.new",
        label: "Start an agent",
        detail: "In a project you pick, ready to type to",
        Icon: IconAgent,
        run: () => void cmd.startAgent(),
    },
    { id: "palette.commands", label: "Browse commands", detail: "Every action and its shortcut", Icon: IconSearch, run: cmd.openCommandPalette },
    { id: "ssh.open", label: "Connect to a host", detail: "Hosts from your SSH config", Icon: IconCommand, run: () => cmd.openPicker("ssh") },
] as const satisfies readonly {
    id: CoreKeybindingActionId;
    label: string;
    detail: string;
    Icon: typeof IconFolder;
    run: () => void;
}[];

export function ExperienceBackdrop({
    label,
    className,
    onClose,
    children,
}: {
    label: string;
    className: string;
    onClose: () => void;
    children: React.ReactNode;
}) {
    useEffect(() => {
        const key = (event: KeyboardEvent) => {
            if (event.key === "Escape") onClose();
        };
        window.addEventListener("keydown", key);
        return () => window.removeEventListener("keydown", key);
    }, [onClose]);
    return (
        <div className="experience-backdrop" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
            <section className={className} role="dialog" aria-modal="true" aria-label={label}>
                {children}
            </section>
        </div>
    );
}

function Frame({ label, onClose, children }: { label: string; onClose: () => void; children: React.ReactNode }) {
    return (
        <ExperienceBackdrop label={label} className="experience-frame" onClose={onClose}>
            <div className="experience-notch" aria-hidden="true" />
            <header>
                <span className="experience-kicker">Sikemux signal deck</span>
                <h1>{label}</h1>
                <button onClick={onClose} aria-label={`Close ${label}`}>
                    esc
                </button>
            </header>
            {children}
        </ExperienceBackdrop>
    );
}

export function Onboarding() {
    const open = useStore((s) => s.onboardingOpen);
    const overrides = useStore((s) => s.keybindingOverrides);
    const profiles = useStore((s) => s.providerProfiles);
    const profileSelections = useStore((s) => s.selectedProviderProfileIds);
    const runtimeProfiles = useMemo(() => selectedAgentRuntimeProfiles(profiles, profileSelections), [profiles, profileSelections]);
    const catalog = useResourceEnabled(open, agentCatalogR, runtimeProfiles);
    const [gitMissing, setGitMissing] = useState(false);
    const dialogRef = useRef<HTMLElement>(null);
    const movesRef = useRef<HTMLDivElement>(null);
    const returnFocusRef = useRef<HTMLElement | null>(null);

    useEffect(() => {
        if (!open) return;
        let disposed = false;
        setGitMissing(false);
        void invoke<IntegrationHealth>("integration_health")
            .then((value) => {
                if (!disposed) setGitMissing(!value.git);
            })
            .catch(() => {});
        return () => {
            disposed = true;
        };
    }, [open]);

    useEffect(() => {
        if (!open) return;
        returnFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
        const frame = window.requestAnimationFrame(() => movesRef.current?.querySelector<HTMLElement>("button")?.focus());
        return () => {
            window.cancelAnimationFrame(frame);
            returnFocusRef.current?.focus();
        };
    }, [open]);

    if (!open) return null;

    const runMove = (run: () => void) => {
        cmd.closeOnboarding();
        run();
    };
    const onKeyDown = (event: React.KeyboardEvent<HTMLElement>) => {
        event.stopPropagation();
        if (event.key === "Escape") {
            event.preventDefault();
            cmd.closeOnboarding();
            return;
        }
        const pressed = WELCOME_MOVES.find((move) => move.id === actionForEvent(event.nativeEvent, overrides));
        if (pressed) {
            event.preventDefault();
            runMove(pressed.run);
            return;
        }
        if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            const moves = [...(movesRef.current?.querySelectorAll<HTMLElement>("button") ?? [])];
            const index = moves.indexOf(document.activeElement as HTMLElement);
            if (index === -1) return;
            event.preventDefault();
            moves[(index + (event.key === "ArrowDown" ? 1 : moves.length - 1)) % moves.length].focus();
            return;
        }
        if (event.key !== "Tab") return;
        const focusable = [...(dialogRef.current?.querySelectorAll<HTMLElement>("button:not([disabled])") ?? [])];
        const first = focusable[0];
        const last = focusable.at(-1);
        if (!first || !last) return;
        const active = document.activeElement;
        if (event.shiftKey && (active === first || active === dialogRef.current)) {
            event.preventDefault();
            last.focus();
        } else if (!event.shiftKey && (active === last || active === dialogRef.current)) {
            event.preventDefault();
            first.focus();
        }
    };
    const agents = [...(catalog.data ?? [])].sort((a, b) => Number(b.available !== false) - Number(a.available !== false));
    const dragWindow = (event: React.MouseEvent<HTMLElement>) => {
        if (event.button !== 0 || event.target !== event.currentTarget) return;
        event.preventDefault();
        void getCurrentWindow()
            .startDragging()
            .catch(() => {});
    };

    return (
        <section
            ref={dialogRef}
            className="welcome"
            role="dialog"
            aria-modal="true"
            aria-labelledby="welcome-title"
            aria-describedby="welcome-description"
            tabIndex={-1}
            onKeyDown={onKeyDown}
            onMouseDown={dragWindow}>
            <ShaderField preset="release" className="welcome-sky" />
            <div className="welcome-body" onMouseDown={dragWindow}>
                <Logo size={64} className="welcome-mark" />
                <h1 id="welcome-title">
                    Welcome to <span>Sikemux</span>
                </h1>
                <p id="welcome-description" className="welcome-lede">
                    Projects, terminals and coding agents in one keyboard-first window.
                </p>

                <div className="welcome-moves" ref={movesRef}>
                    {WELCOME_MOVES.map(({ id, label, detail, Icon, run }) => (
                        <button key={id} type="button" onClick={() => runMove(run)}>
                            <Icon size={16} />
                            <span>
                                <b>{label}</b>
                                <small>{detail}</small>
                            </span>
                            <Kbd>{keybindingLabelForAction(overrides, id)}</Kbd>
                        </button>
                    ))}
                </div>

                <div className="welcome-found" aria-live="polite">
                    <span className="welcome-label">Agents on this machine</span>
                    {catalog.status === "loading" && agents.length === 0 ? (
                        <p className="welcome-muted">Looking on your PATH…</p>
                    ) : agents.length === 0 ? (
                        <p className="welcome-muted">None found. Install Claude Code, Codex or another supported CLI.</p>
                    ) : (
                        <ul className="welcome-agents">
                            {agents.map((agent) => (
                                <li
                                    key={agent.profileId ?? agent.type}
                                    className={agent.available === false ? "is-missing" : ""}
                                    title={agent.available === false ? `${agent.command} is not installed` : agent.command}>
                                    <span className={`agent-glyph ${agent.type}`}>
                                        <AgentIcon type={agent.type} size={18} />
                                    </span>
                                    <span>{agent.label}</span>
                                </li>
                            ))}
                        </ul>
                    )}
                </div>
            </div>

            <footer className="welcome-foot" onMouseDown={dragWindow}>
                {gitMissing && <span className="welcome-warn">git not found: the Git view needs it</span>}
                <button type="button" onClick={() => cmd.closeOnboarding()} aria-label="Close welcome">
                    <Kbd>esc</Kbd> skip
                </button>
            </footer>
        </section>
    );
}

const STALLED_COMMAND_MS = 1_000;
const UI_ACTIVITY_REFRESH_MS = 1_000;
const SLOWEST_COMMANDS_SHOWN = 5;
const REJECTION_MESSAGES_SHOWN = 5;

/** What the interface was doing, so a stall can be read before it becomes a freeze. */
function UiActivityBreadcrumbs({ report }: { report: UiActivityReport | null }) {
    const slowest = useMemo(
        () => (report ? [...report.recent].sort((left, right) => right.ms - left.ms).slice(0, SLOWEST_COMMANDS_SHOWN) : []),
        [report],
    );
    if (!report) return null;
    return (
        <div className="diagnostics-signals diagnostics-activity">
            <span className="experience-kicker">in flight{report.focusPane ? ` · ${report.focusPane} pane focused` : ""}</span>
            {report.inflight.length ? (
                report.inflight.map((entry, index) => (
                    <span key={`${entry.command}-${index}`} className={entry.ageMs >= STALLED_COMMAND_MS ? "is-stalled" : ""}>
                        <b>{entry.command}</b>
                        <small>{Math.round(entry.ageMs)}ms</small>
                    </span>
                ))
            ) : (
                <span>
                    <b>nothing outstanding</b>
                </span>
            )}

            <span className="experience-kicker">slowest recent commands</span>
            {slowest.length ? (
                slowest.map((entry, index) => (
                    <span key={`${entry.command}-${index}`} className={entry.ok ? "" : "is-failed"}>
                        <b>{entry.command}</b>
                        <small>
                            {Math.round(entry.ms)}ms {entry.ok ? "" : "· failed"}
                        </small>
                    </span>
                ))
            ) : (
                <span>
                    <b>no commands yet</b>
                </span>
            )}

            <span className="experience-kicker">top rejection messages</span>
            {report.rejections.length ? (
                report.rejections.slice(0, REJECTION_MESSAGES_SHOWN).map((entry) => (
                    <span key={entry.message} className="is-failed" title={entry.message}>
                        <b>{entry.message}</b>
                        <small>×{entry.count}</small>
                    </span>
                ))
            ) : (
                <span>
                    <b>none</b>
                </span>
            )}
        </div>
    );
}

export function DiagnosticsOverlay() {
    const open = useStore((s) => s.diagnosticsOpen);
    useOccludeNativeViews(open);
    const [snapshot, setSnapshot] = useState<unknown>(null);
    const [error, setError] = useState("");
    const [manifests, setManifests] = useState<ManifestReport | null>(null);
    const [explain, setExplain] = useState<unknown>(null);
    const [uiReport, setUiReport] = useState<UiActivityReport | null>(null);
    const agents = useStore((s) => s.agents);
    const activity = useStore((s) => s.agentActivity);
    const refresh = async () => {
        setError("");
        try {
            const [native, detection] = await Promise.all([nativeDiagnostics(), agentDetectionApi.manifests()]);
            setSnapshot({ browser: browserDiagnostics(), native });
            setManifests(detection);
        } catch (value) {
            setError(value instanceof Error ? value.message : String(value));
        }
    };
    useEffect(() => {
        if (open) void refresh();
    }, [open]);
    useEffect(() => {
        if (!open) return;
        setUiReport(uiActivity.snapshot());
        const timer = window.setInterval(() => setUiReport(uiActivity.snapshot()), UI_ACTIVITY_REFRESH_MS);
        return () => window.clearInterval(timer);
    }, [open]);
    if (!open) return null;
    const text = JSON.stringify(snapshot, null, 2);
    return (
        <Frame label="Runtime diagnostics" onClose={cmd.closeDiagnostics}>
            <p className="experience-deck">
                A redacted operational snapshot. Terminal text, environment values, credentials, and API secrets are never included.
            </p>
            <UiActivityBreadcrumbs report={uiReport} />
            <div className="diagnostics-signals">
                <span className="experience-kicker">agent detection manifests</span>
                {manifests?.manifests.map((item) => (
                    <span key={item.agent}>
                        <b>{item.agent}</b> v{item.version} · {item.source.kind}
                        {item.warning ? " · warning" : ""}
                    </span>
                ))}
                {Object.values(agents).map((agent) => (
                    <button
                        key={agent.id}
                        type="button"
                        disabled={agent.launchState === "dormant"}
                        onClick={() =>
                            void agentDetectionApi
                                .explain(agent.id)
                                .then(setExplain)
                                .catch((value) => setError(String(value)))
                        }>
                        <b>{agent.title}</b>
                        <span>{agent.launchState === "dormant" ? "dormant" : (activity[agent.id]?.state ?? "unknown")}</span>
                        <small>explain</small>
                    </button>
                ))}
            </div>
            {explain != null && <pre className="diagnostics-json diagnostics-explain">{JSON.stringify(explain, null, 2)}</pre>}
            {error ? <p className="experience-error">{error}</p> : <pre className="diagnostics-json">{text || "Collecting…"}</pre>}
            <footer>
                <button
                    onClick={() =>
                        void agentDetectionApi
                            .reload()
                            .then(setManifests)
                            .catch((value) => setError(String(value)))
                    }>
                    Reload manifests
                </button>
                <button onClick={() => void refresh()}>Refresh</button>
                <button onClick={() => void copyText(text)}>Copy JSON</button>
                <button
                    disabled={snapshot == null}
                    onClick={() =>
                        void exportDiagnosticsSnapshot(snapshot).catch((value) => setError(value instanceof Error ? value.message : String(value)))
                    }>
                    Save JSON
                </button>
            </footer>
        </Frame>
    );
}
