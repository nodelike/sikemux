import { useEffect, useMemo, useRef, useState } from "react";
import Markdown from "react-markdown";
import { getVersion } from "@tauri-apps/api/app";
import { invokeCommand as invoke } from "../api/invoke";
import { browserDiagnostics, exportDiagnosticsSnapshot, nativeDiagnostics } from "../lib/diagnostics";
import { useResource } from "../state/resources";
import { agentCatalogR } from "../state/resources.defs";
import { useStore } from "../state/store";
import * as cmd from "../state/commands";
import { installPendingUpdate, isUpdateBusy, updateStatusLabel } from "../api/updater";
import { agentDetectionApi, type ManifestReport } from "../api/agentDetection";
import { selectedAgentRuntimeProfiles } from "../agentProfiles";
import {
    getKeybindingAction,
    keybindingLabelForAction,
    matchesKeybinding,
    resolvedKeybinding,
    type KeybindingActionId,
    type KeybindingOverrides,
} from "../keybindings";
import { OnboardingStage, type OnboardingOverlay, type OnboardingRegion } from "./OnboardingStage";
import { useShaderField } from "../hooks/useShaderField";
import { Logo } from "./Icons";
import type { AgentPresentationState } from "../state/types";

interface IntegrationHealth {
    shell: string;
    git: boolean;
    aws: boolean;
    rnd: boolean;
}

const ONBOARDING_STEPS = [
    { kicker: "One cockpit", title: "Everything you ship, one signal away" },
    { kicker: "Muscle memory", title: "Open anything without breaking flow" },
    { kicker: "Agent signals", title: "Know when to watch, help, or move on" },
    { kicker: "Launch ready", title: "Your first move is already mapped" },
] as const;

/** Scene one: each entry points the miniature at the region it describes. */
const ONBOARDING_REGIONS = [
    { id: "rail", label: "Sessions rail", detail: "Every project, host, and cloud you have open" },
    { id: "stage", label: "Work stage", detail: "Terminals, editors, diffs, and requests in split panes" },
    { id: "agents", label: "Agent rail", detail: "Coding agents parked beside the code they touch" },
] as const satisfies readonly { id: OnboardingRegion; label: string; detail: string }[];

/** Scene two: the bindings the tour asks you to actually press. */
const ONBOARDING_KEYS = [
    { id: "session.open", overlay: "sessions" },
    { id: "palette.commands", overlay: "commands" },
] as const satisfies readonly { id: KeybindingActionId; overlay: OnboardingOverlay }[];

/** Scene three: the three states worth reacting to, in the order they usually happen. */
const ONBOARDING_SIGNALS = [
    { state: "working", label: "Working", detail: "Let it cook" },
    { state: "blocked", label: "Needs input", detail: "Unblock it" },
    { state: "done", label: "Ready", detail: "Review the result" },
] as const satisfies readonly { state: AgentPresentationState; label: string; detail: string }[];

/** Scene four: real first moves. Picking one ends the tour and runs the action. */
const ONBOARDING_LAUNCHES = [
    { id: "project.open", label: "Open a project", overlay: null, region: "rail", run: () => cmd.openPicker("projects") },
    { id: "session.open", label: "Open any session", overlay: "sessions", region: null, run: () => cmd.openPicker("all") },
    { id: "palette.commands", label: "Open the command deck", overlay: "commands", region: null, run: cmd.openCommandPalette },
] as const satisfies readonly {
    id: KeybindingActionId;
    label: string;
    overlay: OnboardingOverlay | null;
    region: OnboardingRegion | null;
    run: () => void;
}[];

const SIGNAL_CYCLE_MS = 2400;
const KEY_DEMO_MS = 4200;

function Frame({ label, onClose, children }: { label: string; onClose: () => void; children: React.ReactNode }) {
    useEffect(() => {
        const key = (event: KeyboardEvent) => {
            if (event.key === "Escape") onClose();
        };
        window.addEventListener("keydown", key);
        return () => window.removeEventListener("keydown", key);
    }, [onClose]);
    return (
        <div className="experience-backdrop" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
            <section className="experience-frame" role="dialog" aria-modal="true" aria-label={label}>
                <div className="experience-notch" aria-hidden="true" />
                <header>
                    <span className="experience-kicker">Sikemux signal deck</span>
                    <h1>{label}</h1>
                    <button onClick={onClose} aria-label={`Close ${label}`}>
                        esc
                    </button>
                </header>
                {children}
            </section>
        </div>
    );
}

function pressedOnboardingKey(event: KeyboardEvent, overrides: KeybindingOverrides): KeybindingActionId | null {
    for (const entry of ONBOARDING_KEYS) {
        const binding = resolvedKeybinding(overrides, entry.id);
        if (binding && matchesKeybinding(event, binding)) return entry.id;
    }
    return null;
}

export function Onboarding() {
    const open = useStore((s) => s.onboardingOpen);
    const overrides = useStore((s) => s.keybindingOverrides);
    const profiles = useStore((s) => s.providerProfiles);
    const profileSelections = useStore((s) => s.selectedProviderProfileIds);
    const runtimeProfiles = useMemo(() => selectedAgentRuntimeProfiles(profiles, profileSelections), [profiles, profileSelections]);
    const catalog = useResource(agentCatalogR, runtimeProfiles);
    const [health, setHealth] = useState<IntegrationHealth | null>(null);
    const [healthUnavailable, setHealthUnavailable] = useState(false);
    const [step, setStep] = useState(0);
    const [direction, setDirection] = useState<"forward" | "back">("forward");
    const onboardingFieldRef = useShaderField<HTMLSpanElement>("onboarding", open);
    const [region, setRegion] = useState<OnboardingRegion | null>(null);
    const [tried, setTried] = useState<KeybindingActionId[]>([]);
    const [demo, setDemo] = useState<KeybindingActionId | null>(null);
    const [signal, setSignal] = useState<AgentPresentationState>("working");
    const [signalPinned, setSignalPinned] = useState(false);
    const [launch, setLaunch] = useState<number | null>(null);
    const dialogRef = useRef<HTMLElement>(null);
    const returnFocusRef = useRef<HTMLElement | null>(null);

    useEffect(() => {
        if (!open) return;
        let disposed = false;
        setHealth(null);
        setHealthUnavailable(false);
        void invoke<IntegrationHealth>("integration_health")
            .then((value) => {
                if (!disposed) setHealth(value);
            })
            .catch(() => {
                if (!disposed) setHealthUnavailable(true);
            });
        return () => {
            disposed = true;
        };
    }, [open]);

    useEffect(() => {
        if (!open) return;
        returnFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
        setStep(0);
        setDirection("forward");
        setTried([]);
        return () => {
            returnFocusRef.current?.focus();
        };
    }, [open]);

    useEffect(() => {
        if (!open) return;
        const frame = window.requestAnimationFrame(() => dialogRef.current?.focus());
        return () => window.cancelAnimationFrame(frame);
    }, [open]);

    // Each scene starts from a clean pointer so the miniature never carries a
    // highlight that the new copy does not explain.
    useEffect(() => {
        setRegion(null);
        setDemo(null);
        setLaunch(null);
        setSignal("working");
        setSignalPinned(false);
    }, [step]);

    // Scene three narrates the rail by running it: the states advance on their
    // own until the reader takes over by pointing at one.
    useEffect(() => {
        if (!open || step !== 2 || signalPinned) return;
        const timer = window.setInterval(() => {
            setSignal((current) => {
                const index = ONBOARDING_SIGNALS.findIndex((entry) => entry.state === current);
                return ONBOARDING_SIGNALS[(index + 1) % ONBOARDING_SIGNALS.length].state;
            });
        }, SIGNAL_CYCLE_MS);
        return () => window.clearInterval(timer);
    }, [open, step, signalPinned]);

    // The demo overlay is a preview, not a mode — it always retracts by itself.
    useEffect(() => {
        if (!demo) return;
        const timer = window.setTimeout(() => setDemo(null), KEY_DEMO_MS);
        return () => window.clearTimeout(timer);
    }, [demo]);

    const tryKey = (id: KeybindingActionId) => {
        setDemo(id);
        setTried((current) => (current.includes(id) ? current : [...current, id]));
    };

    // Scene two only works if the real binding does something here. The keymap
    // suppresses every action while onboarding is open, so the tour is free to
    // claim these presses.
    useEffect(() => {
        if (!open || step !== 1) return;
        const onKey = (event: KeyboardEvent) => {
            const id = pressedOnboardingKey(event, overrides);
            if (!id) return;
            event.preventDefault();
            tryKey(id);
        };
        window.addEventListener("keydown", onKey);
        return () => window.removeEventListener("keydown", onKey);
    }, [open, step, overrides]);

    if (!open) return null;

    const goTo = (next: number) => {
        const bounded = Math.max(0, Math.min(ONBOARDING_STEPS.length - 1, next));
        if (bounded === step) return;
        setDirection(bounded < step ? "back" : "forward");
        setStep(bounded);
    };
    const next = () => {
        if (step === ONBOARDING_STEPS.length - 1) cmd.closeOnboarding();
        else goTo(step + 1);
    };
    const runLaunch = (run: () => void) => {
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
        if (event.key === "ArrowRight") {
            event.preventDefault();
            next();
            return;
        }
        if (event.key === "ArrowLeft") {
            event.preventDefault();
            goTo(step - 1);
            return;
        }
        if (event.key !== "Tab") return;
        const focusable = [
            ...(dialogRef.current?.querySelectorAll<HTMLElement>("button:not([disabled]), [href], [tabindex]:not([tabindex='-1'])") ?? []),
        ];
        if (focusable.length === 0) {
            event.preventDefault();
            dialogRef.current?.focus();
            return;
        }
        const first = focusable[0];
        const last = focusable.at(-1)!;
        const active = document.activeElement;
        if (event.shiftKey && (active === first || active === dialogRef.current)) {
            event.preventDefault();
            last.focus();
        } else if (!event.shiftKey && active === last) {
            event.preventDefault();
            first.focus();
        } else if (!event.shiftKey && active === dialogRef.current) {
            event.preventDefault();
            first.focus();
        }
    };
    const shortcut = (id: KeybindingActionId) => ({
        action: getKeybindingAction(id),
        label: keybindingLabelForAction(overrides, id),
    });
    const detectedAgents = catalog.data?.filter((agent) => agent.available !== false).map((agent) => agent.label) ?? [];
    const healthSignals = health
        ? [
              { label: `shell ${health.shell || "unknown"}`, ready: !!health.shell },
              { label: "git", ready: health.git },
              { label: "aws", ready: health.aws },
              { label: "rundeck", ready: health.rnd },
          ]
        : [];
    const commandRows = (["pane.splitRow", "pane.zoom", "settings.toggle"] as const).map((id) => ({
        label: getKeybindingAction(id).label,
        kbd: keybindingLabelForAction(overrides, id),
    }));
    const hoveredLaunch = launch === null ? null : ONBOARDING_LAUNCHES[launch];
    const stageRegion = step === 0 ? region : step === 2 ? "agents" : (hoveredLaunch?.region ?? null);
    const stageOverlay = step === 1 ? (ONBOARDING_KEYS.find((entry) => entry.id === demo)?.overlay ?? null) : (hoveredLaunch?.overlay ?? null);

    return (
        <div className="experience-backdrop onboarding-backdrop" role="presentation">
            <section
                ref={dialogRef}
                className="experience-frame onboarding-frame"
                role="dialog"
                aria-modal="true"
                aria-labelledby="onboarding-title"
                aria-describedby="onboarding-description"
                tabIndex={-1}
                onKeyDown={onKeyDown}>
                <span className="onb-field" aria-hidden="true" ref={onboardingFieldRef} />
                <header className="onboarding-header">
                    <div>
                        <Logo size={13} className="onboarding-mark" />
                        <span className="experience-kicker">Sikemux · first run</span>
                        <span className="onboarding-step-count">
                            {String(step + 1).padStart(2, "0")} / {String(ONBOARDING_STEPS.length).padStart(2, "0")}
                        </span>
                    </div>
                    <button className="onboarding-skip" onClick={() => cmd.closeOnboarding()} aria-label="Skip onboarding tour">
                        Skip tour <span aria-hidden="true">esc</span>
                    </button>
                </header>

                <div className="onboarding-progress" aria-label={`Onboarding step ${step + 1} of ${ONBOARDING_STEPS.length}`}>
                    {ONBOARDING_STEPS.map((item, index) => (
                        <button
                            key={item.kicker}
                            className={index === step ? "is-current" : index < step ? "is-complete" : ""}
                            onClick={() => goTo(index)}
                            aria-label={`Go to step ${index + 1}: ${item.kicker}`}
                            aria-current={index === step ? "step" : undefined}>
                            <span />
                        </button>
                    ))}
                </div>

                <div className="onboarding-scene">
                    <div key={step} className="onboarding-copy" data-direction={direction}>
                        <span className="experience-kicker">{ONBOARDING_STEPS[step].kicker}</span>
                        <h1 id="onboarding-title">{ONBOARDING_STEPS[step].title}</h1>

                        {step === 0 && (
                            <>
                                <p id="onboarding-description">
                                    Projects, terminals, cloud consoles, API work, and coding agents share one keyboard-first window. Point at a
                                    region to find it.
                                </p>
                                <div className="onboarding-regions" onMouseLeave={() => setRegion(null)}>
                                    {ONBOARDING_REGIONS.map((item) => (
                                        <button
                                            key={item.id}
                                            type="button"
                                            className={region === item.id ? "is-on" : ""}
                                            onMouseEnter={() => setRegion(item.id)}
                                            onFocus={() => setRegion(item.id)}
                                            onBlur={() => setRegion((current) => (current === item.id ? null : current))}
                                            onClick={() => setRegion(item.id)}>
                                            <b>{item.label}</b>
                                            <small>{item.detail}</small>
                                        </button>
                                    ))}
                                </div>
                            </>
                        )}

                        {step === 1 && (
                            <>
                                <p id="onboarding-description">
                                    Two bindings carry most of the navigation. Press them now — the tour is listening, and nothing behind it will
                                    move.
                                </p>
                                <div className="onboarding-keys">
                                    {ONBOARDING_KEYS.map((entry) => {
                                        const { action, label } = shortcut(entry.id);
                                        const done = tried.includes(entry.id);
                                        return (
                                            <button
                                                key={entry.id}
                                                type="button"
                                                className={`onboarding-key${done ? " is-done" : ""}${demo === entry.id ? " is-live" : ""}`}
                                                onClick={() => tryKey(entry.id)}>
                                                <kbd>{label}</kbd>
                                                <span>
                                                    <b>{action.label}</b>
                                                    <small>{action.detail}</small>
                                                </span>
                                                <em>{done ? "✓ tried" : "press it"}</em>
                                            </button>
                                        );
                                    })}
                                </div>
                                <p className="onboarding-aside" aria-live="polite">
                                    {tried.length === ONBOARDING_KEYS.length
                                        ? "That is the whole navigation model. Every binding is remappable in Settings › Keybindings."
                                        : "Prefer the mouse? Click a card to watch the same thing happen."}
                                </p>
                            </>
                        )}

                        {step === 2 && (
                            <>
                                <p id="onboarding-description">
                                    The agent rail stays quiet until something needs you. Colour tells you what kind of attention to give.
                                </p>
                                <div className="onboarding-signals">
                                    {ONBOARDING_SIGNALS.map((item) => (
                                        <button
                                            key={item.state}
                                            type="button"
                                            className={`state-${item.state}${signal === item.state ? " is-on" : ""}`}
                                            onMouseEnter={() => {
                                                setSignal(item.state);
                                                setSignalPinned(true);
                                            }}
                                            onFocus={() => {
                                                setSignal(item.state);
                                                setSignalPinned(true);
                                            }}
                                            onClick={() => {
                                                setSignal(item.state);
                                                setSignalPinned(true);
                                            }}>
                                            <i />
                                            <b>{item.label}</b>
                                            <span>{item.detail}</span>
                                        </button>
                                    ))}
                                </div>
                                <div className="onboarding-detected">
                                    <span>Detected here</span>
                                    <b>
                                        {catalog.status === "loading"
                                            ? "Checking PATH…"
                                            : detectedAgents.length
                                              ? detectedAgents.join(" · ")
                                              : "Add a supported agent whenever you’re ready"}
                                    </b>
                                </div>
                            </>
                        )}

                        {step === 3 && (
                            <>
                                <p id="onboarding-description">
                                    Pick a first move and the tour gets out of the way. Replay it any time from the command deck or Settings.
                                </p>
                                <div className="onboarding-launch" onMouseLeave={() => setLaunch(null)}>
                                    {ONBOARDING_LAUNCHES.map((item, index) => (
                                        <button
                                            key={item.label}
                                            type="button"
                                            className={launch === index ? "is-on" : ""}
                                            onMouseEnter={() => setLaunch(index)}
                                            onFocus={() => setLaunch(index)}
                                            onBlur={() => setLaunch((current) => (current === index ? null : current))}
                                            onClick={() => runLaunch(item.run)}>
                                            <b>{item.label}</b>
                                            <kbd>{keybindingLabelForAction(overrides, item.id)}</kbd>
                                        </button>
                                    ))}
                                </div>
                                <div className="onboarding-health" aria-live="polite">
                                    {healthSignals.length ? (
                                        healthSignals.map((item) => (
                                            <span key={item.label} className={item.ready ? "is-ready" : "is-muted"}>
                                                {item.label} {item.ready ? "ready" : "missing"}
                                            </span>
                                        ))
                                    ) : (
                                        <span className={healthUnavailable ? "is-muted" : "is-checking"}>
                                            {healthUnavailable ? "Local tool check unavailable — setup can continue" : "Checking local tools…"}
                                        </span>
                                    )}
                                </div>
                            </>
                        )}
                    </div>

                    <OnboardingStage
                        scene={step}
                        region={stageRegion}
                        overlay={stageOverlay}
                        agentState={step === 2 ? signal : step === 3 ? "done" : "working"}
                        commandRows={commandRows}
                    />
                </div>

                <span className="onboarding-sr-only" role="status" aria-live="polite">
                    Step {step + 1} of {ONBOARDING_STEPS.length}: {ONBOARDING_STEPS[step].title}
                </span>

                <footer className="onboarding-footer">
                    <span className="onboarding-key-hint">
                        <kbd>←</kbd>
                        <kbd>→</kbd> to move between steps
                    </span>
                    <div>
                        {step > 0 && <button onClick={() => goTo(step - 1)}>Back</button>}
                        <button className="primary" onClick={next}>
                            {step === ONBOARDING_STEPS.length - 1 ? "Enter Sikemux" : "Continue"}
                            <span aria-hidden="true"> {step === ONBOARDING_STEPS.length - 1 ? "↵" : "→"}</span>
                        </button>
                    </div>
                </footer>
            </section>
        </div>
    );
}

export function DiagnosticsOverlay() {
    const open = useStore((s) => s.diagnosticsOpen);
    const [snapshot, setSnapshot] = useState<unknown>(null);
    const [error, setError] = useState("");
    const [manifests, setManifests] = useState<ManifestReport | null>(null);
    const [explain, setExplain] = useState<unknown>(null);
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
    if (!open) return null;
    const text = JSON.stringify(snapshot, null, 2);
    return (
        <Frame label="Runtime diagnostics" onClose={cmd.closeDiagnostics}>
            <p className="experience-deck">
                A redacted operational snapshot. Terminal text, environment values, credentials, and API secrets are never included.
            </p>
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
                <button onClick={() => void navigator.clipboard.writeText(text)}>Copy JSON</button>
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

export function WhatsNewOverlay() {
    const open = useStore((s) => s.whatsNewOpen);
    const pending = useStore((s) => s.pendingUpdate);
    const installedNotes = useStore((s) => s.lastReleaseNotes);
    const [version, setVersion] = useState("");
    useEffect(() => {
        if (open) void getVersion().then(setVersion);
    }, [open]);
    if (!open) return null;
    const updateBusy = pending ? isUpdateBusy(pending.state) : false;
    return (
        <Frame label="What’s new" onClose={cmd.closeWhatsNew}>
            <p className="experience-deck">
                You are on Sikemux v{version || "…"}. Release notes stay reachable here instead of disappearing into an update tooltip.
            </p>
            <div className="release-notes">
                <Markdown skipHtml>
                    {pending?.notes ||
                        installedNotes?.notes ||
                        (pending
                            ? `Version ${pending.version} is ready.`
                            : installedNotes
                              ? `Updated to ${installedNotes.version}.`
                              : "You are up to date. No newer release notes are available yet.")}
                </Markdown>
            </div>
            <footer>
                {pending && (
                    <button className="primary" disabled={updateBusy} onClick={() => void installPendingUpdate()}>
                        {updateBusy
                            ? updateStatusLabel(pending)
                            : pending.state === "error"
                              ? `Retry v${pending.version}`
                              : `Install v${pending.version}`}
                    </button>
                )}
            </footer>
        </Frame>
    );
}
