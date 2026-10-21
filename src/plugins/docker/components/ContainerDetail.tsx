import type { ReactNode } from "react";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { Seg, SkeletonRows, StaleNotice } from "../../../plugin-api/ui";
import { formatAgo } from "../../../plugin-api/format";
import type { ContainerDetail as Detail } from "../api";
import { usePoll } from "../hooks";
import { dockerContainerR } from "../resources";
import { shortImage } from "../format";
import type { ContainerTab } from "../state";
import { ContainerActions } from "./ContainerActions";
import { Ports, StateDot } from "./parts";

const TABS = [
    { value: "logs", label: "Logs" },
    { value: "inspect", label: "Inspect" },
    { value: "stats", label: "Stats" },
] as const;

const DETAIL_POLL_MS = 4_000;

export function ContainerDetail({
    active,
    name,
    tab,
    onTab,
    onBack,
    children,
}: {
    active: boolean;
    name: string;
    tab: ContainerTab;
    onTab: (tab: ContainerTab) => void;
    onBack: () => void;
    /** What the Logs and Stats tabs show, which the pane draws. */
    children?: (detail: Detail) => ReactNode;
}) {
    const detail = useResourceEnabled(active, dockerContainerR, name);
    usePoll(active, DETAIL_POLL_MS, detail.refresh);
    if (detail.status === "error" && !detail.data) {
        return (
            <section className="docker-section">
                <button type="button" className="docker-back" onClick={onBack}>
                    ← Containers
                </button>
                <p className="docker-note">{detail.error}</p>
            </section>
        );
    }
    if (!detail.data) return <SkeletonRows rows={6} label={`Loading ${name}`} />;
    const container = detail.data;
    return (
        <section className="docker-section docker-detail" aria-label={container.name}>
            <button type="button" className="docker-back" onClick={onBack}>
                ← Containers
            </button>
            <StaleNotice handle={detail} />
            <header className="docker-detail-head">
                <StateDot container={{ state: container.state.status, status: summary(container) }} />
                <h2 className="docker-detail-name">{container.name}</h2>
                <span className="docker-muted">{summary(container)}</span>
                <ContainerActions container={container} onRemoved={onBack} />
            </header>
            <div className="docker-detail-facts">
                <span className="docker-mono" title={container.image}>
                    {shortImage(container.image)}
                </span>
                {container.compose ? (
                    <span className="docker-chip">
                        {container.compose.project} · {container.compose.service}
                    </span>
                ) : null}
                {container.ports.length > 0 ? <Ports ports={container.ports} /> : null}
            </div>
            <Seg label="View" options={TABS} value={tab} onChange={onTab} />
            {tab === "inspect" ? <Inspect container={container} /> : children?.(container)}
        </section>
    );
}

/** How the engine would put it: running for how long, or how it ended. */
export function summary(container: Detail): string {
    const { state } = container;
    if (state.running) {
        const health = state.health ? ` (${state.health.status})` : "";
        const since = state.startedAt ? ` since ${formatAgo(Date.parse(state.startedAt))}` : "";
        return `${state.status === "paused" ? "Paused" : "Running"}${since}${health}`;
    }
    if (state.status === "created") return "Created, never started";
    const when = state.finishedAt ? ` ${formatAgo(Date.parse(state.finishedAt))}` : "";
    return `Exited (${state.exitCode})${when}${state.oomKilled ? " — out of memory" : ""}`;
}

function Inspect({ container }: { container: Detail }) {
    const { state } = container;
    const command = [...container.entrypoint, ...container.command].join(" ");
    return (
        <div className="docker-inspect">
            <Facts title="State">
                <Fact label="Status">{state.status}</Fact>
                {!state.running && state.status !== "created" ? <Fact label="Exit code">{String(state.exitCode)}</Fact> : null}
                {state.error ? <Fact label="Error">{state.error}</Fact> : null}
                {state.oomKilled ? <Fact label="Memory">Killed for running out of memory</Fact> : null}
                {state.startedAt ? <Fact label="Started">{new Date(state.startedAt).toLocaleString()}</Fact> : null}
                {state.finishedAt && !state.running ? <Fact label="Finished">{new Date(state.finishedAt).toLocaleString()}</Fact> : null}
                <Fact label="Restarts">
                    {container.restartCount} · policy {container.restartPolicy}
                </Fact>
            </Facts>
            {state.health ? (
                <Facts title="Health">
                    <Fact label="Status">
                        {state.health.status}
                        {state.health.failingStreak > 0 ? ` · ${state.health.failingStreak} failed in a row` : ""}
                    </Fact>
                    {state.health.lastOutput.filter(Boolean).length > 0 ? (
                        <pre className="docker-pre">{state.health.lastOutput.filter(Boolean).join("\n")}</pre>
                    ) : null}
                </Facts>
            ) : null}
            <Facts title="Runs">
                <Fact label="Command">
                    <code>{command || "—"}</code>
                </Fact>
                {container.workingDir ? (
                    <Fact label="Folder">
                        <code>{container.workingDir}</code>
                    </Fact>
                ) : null}
                <Fact label="Image ID">
                    <code>{container.imageId.replace(/^sha256:/, "").slice(0, 12)}</code>
                </Fact>
                <Fact label="ID">
                    <code>{container.id}</code>
                </Fact>
            </Facts>
            {container.mounts.length > 0 ? (
                <Facts title="Mounts">
                    {container.mounts.map((mount) => (
                        <Fact key={mount.destination} label={mount.destination}>
                            <code>{mount.volume ?? mount.source}</code>
                            <span className="docker-muted">
                                {" "}
                                · {mount.kind}
                                {mount.writable ? "" : " · read-only"}
                            </span>
                        </Fact>
                    ))}
                </Facts>
            ) : null}
            {container.networks.length > 0 ? (
                <Facts title="Networks">
                    {container.networks.map((network) => (
                        <Fact key={network.name} label={network.name}>
                            {network.ip ?? "no address"}
                            {network.aliases.length > 0 ? <span className="docker-muted"> · {network.aliases.join(", ")}</span> : null}
                        </Fact>
                    ))}
                </Facts>
            ) : null}
            <Facts title="Environment" note="Names only. Sikemux never reads their values.">
                {container.env.length === 0 ? (
                    <span className="docker-muted">None</span>
                ) : (
                    <div className="docker-names">
                        {container.env.map((name) => (
                            <code key={name}>{name}</code>
                        ))}
                    </div>
                )}
            </Facts>
            {Object.keys(container.labels).length > 0 ? (
                <details className="docker-labels">
                    <summary>Labels ({Object.keys(container.labels).length})</summary>
                    {Object.entries(container.labels).map(([key, value]) => (
                        <Fact key={key} label={key}>
                            <code>{value}</code>
                        </Fact>
                    ))}
                </details>
            ) : null}
        </div>
    );
}

function Facts({ title, note, children }: { title: string; note?: string; children: ReactNode }) {
    return (
        <section className="docker-facts" aria-label={title}>
            <h3>{title}</h3>
            {note ? <p className="docker-muted docker-facts-note">{note}</p> : null}
            {children}
        </section>
    );
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
    return (
        <div className="docker-fact">
            <span className="docker-fact-label">{label}</span>
            <span className="docker-fact-value">{children}</span>
        </div>
    );
}
