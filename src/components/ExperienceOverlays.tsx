import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { browserDiagnostics, nativeDiagnostics } from "../lib/diagnostics";
import { useResource } from "../state/resources";
import { agentCatalogR } from "../state/resources.defs";
import { useStore } from "../state/store";
import * as cmd from "../state/commands";
import { installPendingUpdate } from "../api/updater";
import { agentDetectionApi, type ManifestReport } from "../api/agentDetection";

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

export function Onboarding() {
    const open = useStore((s) => s.onboardingOpen);
    const catalog = useResource(agentCatalogR);
    const [health, setHealth] = useState<{ shell: string; git: boolean; aws: boolean; rnd: boolean } | null>(null);
    useEffect(() => {
        if (open) void invoke<typeof health>("integration_health").then(setHealth);
    }, [open]);
    if (!open) return null;
    return (
        <Frame label="Ready for the first run" onClose={() => cmd.closeOnboarding()}>
            <p className="experience-deck">
                Open a project, launch an agent, then let the semantic signal notch tell you when it is working, blocked, or ready.
            </p>
            <div className="experience-list">
                <div>
                    <b>01</b>
                    <span>Open anything</span>
                    <small>⌥S opens projects, commands, SSH, AWS, Rundeck, and Bruno.</small>
                </div>
                <div>
                    <b>02</b>
                    <span>Detected agents</span>
                    <small>
                        {catalog.status === "loading"
                            ? "Checking PATH…"
                            : catalog.data?.length
                              ? catalog.data.map((a) => a.label).join(" · ")
                              : "No supported agent CLI is currently on PATH."}
                    </small>
                </div>
                <div>
                    <b>03</b>
                    <span>Command deck</span>
                    <small>⌘⇧P / Ctrl⇧P searches every built-in action and your own commands.</small>
                </div>
                <div>
                    <b>04</b>
                    <span>Safe restart</span>
                    <small>Known agent sessions return as dormant tabs; only your Resume click starts them.</small>
                </div>
                <div>
                    <b>05</b>
                    <span>Integration health</span>
                    <small>
                        {health
                            ? `shell ${health.shell || "unknown"} · git ${health.git ? "ready" : "missing"} · aws ${health.aws ? "ready" : "missing"} · rnd ${health.rnd ? "ready" : "missing"}`
                            : "Checking local tools…"}
                    </small>
                </div>
            </div>
            <footer>
                <button className="primary" onClick={() => cmd.closeOnboarding()}>
                    Enter Sikemux
                </button>
            </footer>
        </Frame>
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
    return (
        <Frame label="What’s new" onClose={cmd.closeWhatsNew}>
            <p className="experience-deck">
                You are on Sikemux v{version || "…"}. Release notes stay reachable here instead of disappearing into an update tooltip.
            </p>
            <div className="release-notes">
                {pending?.notes ||
                    installedNotes?.notes ||
                    (pending
                        ? `Version ${pending.version} is ready.`
                        : installedNotes
                          ? `Updated to ${installedNotes.version}.`
                          : "You are up to date. No newer release notes are available yet.")}
            </div>
            <footer>
                {pending && (
                    <button className="primary" disabled={pending.state === "installing"} onClick={() => void installPendingUpdate()}>
                        {pending.state === "installing" ? "Installing…" : `Install v${pending.version}`}
                    </button>
                )}
            </footer>
        </Frame>
    );
}
